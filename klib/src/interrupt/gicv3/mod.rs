pub(self) mod lpi_alloc;
pub mod registers;

use core::{
    alloc::Layout,
    arch::asm,
    fmt::Debug,
    ops::Add,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, AtomicU8, Ordering},
};

use aarch64_cpu::{
    asm::{
        barrier::{self, dsb, isb},
        sev, wfe,
    },
    registers::ReadWriteable as TRW,
};
use alloc::{boxed::Box, collections::btree_map::BTreeMap, vec, vec::Vec};
use atomic_refcell::AtomicRefCell;
use mars_models::memory::registers::volatile::{
    PureReadable, PureWriteable, RPureReadWrite, Readable, Writeable,
};

use crate::{
    allocator_support::KernelAddressTranslator,
    cache::clean_dcache_range,
    cpu_interface::CpuIdLogical,
    guard::InterruptGuard,
    interrupt::{
        GicrRegisters, GitsRegisters,
        gicv3::{
            lpi_alloc::LpiAllocator,
            registers::gic::{
                GicBitfield64, GicrPropBar, GicrTyper, GitsBaser, GitsCbasER, GitsCreadr, GitsCtlr,
                GitsCwriter, GitsTyper, ItsCmdWord0, ItsDiscardWord1, ItsInvWord1, ItsInvallWord2,
                ItsMapcWord2, ItsMapdWord1, ItsMapdWord2, ItsMaptiWord1, ItsMaptiWord2,
                ItsSyncWord2, LpiProp,
            },
        },
    },
    pm::page::mapper::AddressTranslator,
    strange::KernelPtr48,
    sync::{FairSpinlock, RwLock},
    this_cpu,
};

use super::{
    GicdRegisters, InterruptController, InterruptError, InterruptInterface, Result,
    gicv3::registers::gic::{GicdCtlr, GicrCtlr, GicrWaker},
};

use self::registers::icc_sre_el1::ICC_SRE_EL1;

pub const ITS_CACHEABILITY: u8 = 0b101; // normal wb
pub const ITS_SHAREABILITY: u8 = 0b01; // inner shareable
pub const ITS_PAGE_SHIFT: u32 = 12;
pub const ITS_PAGE_SIZE: usize = 2usize.pow(ITS_PAGE_SHIFT); // 4K
pub const ITS_PAGE_ALIGN: usize = 2usize.pow(16); // 64K
pub const LPI_START: u32 = 8192;
pub const MAX_LPI_ID: u32 = 2u32.pow(14 + 1) - 1;

static INIT_STATE: AtomicU8 = AtomicU8::new(0);
type IrqHandlerFnPtr = KernelPtr48<fn(u32) -> Result<()>>;

#[derive(Debug, Copy, Clone)]
pub enum IrqTarget {
    Distributor,
    Redistributor,
}

#[derive(Debug, Copy, Clone)]
#[repr(align(8))]
pub struct IrqHandler {
    #[allow(unused, reason = "planned feature")]
    target: IrqTarget,
    dispatch_fn: IrqHandlerFnPtr,
}

struct ItsCmdQueue {
    base: NonNull<u8>,
    write_offset: usize,
    num_entries: usize,
}

// safe to share the buffer across threads
unsafe impl Send for ItsCmdQueue {}
unsafe impl Sync for ItsCmdQueue {}

struct IttAllocation(pub (NonNull<u8>, Layout));

// safe to share the descriptor across threads
unsafe impl Send for IttAllocation {}
unsafe impl Sync for IttAllocation {}

pub struct GicV3<'a, I: InterruptInterface + Send + Sync> {
    pub distributor: &'a GicdRegisters,
    pub redistributors: Vec<AtomicPtr<GicrRegisters>>,
    pub its: Option<AtomicPtr<GitsRegisters>>,
    pub iface: I,
    interrupt_handlers: AtomicRefCell<Box<[Option<IrqHandler>; 1020]>>,
    lpi_handlers: RwLock<BTreeMap<u32, IrqHandler>>,
    device_itts: AtomicRefCell<BTreeMap<u32, IttAllocation>>,
    its_cmd_queue: FairSpinlock<Option<ItsCmdQueue>>,
    lpi_prop_table: AtomicRefCell<Option<u64>>,

    lpi_alloc: FairSpinlock<LpiAllocator>,
    lpi_to_event: FairSpinlock<BTreeMap<u32, (u32, u32, u32)>>,
    event_mappings: FairSpinlock<BTreeMap<(u32, u32), u32>>,
}

impl<I: InterruptInterface + Send + Sync> Debug for GicV3<'_, I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GicV3").finish()
    }
}

impl<'a, I: InterruptInterface + Send + Sync> GicV3<'a, I> {
    pub fn new(
        distributor: &'a mut GicdRegisters,
        redists: Vec<AtomicPtr<GicrRegisters>>,
        its: Option<&'a mut GitsRegisters>,
        iface: I,
    ) -> Self {
        let handlers: Box<[Option<IrqHandler>]> = vec![None; 1020].into_boxed_slice();

        Self {
            distributor,
            redistributors: redists,
            its: its.map(|r| AtomicPtr::new(r as *mut GitsRegisters)),
            iface,
            interrupt_handlers: AtomicRefCell::new(handlers.try_into().unwrap()),
            lpi_handlers: RwLock::new(BTreeMap::new()),
            device_itts: AtomicRefCell::new(BTreeMap::new()),
            its_cmd_queue: FairSpinlock::new(None),
            lpi_prop_table: AtomicRefCell::new(None),

            lpi_alloc: FairSpinlock::new(LpiAllocator::new(LPI_START, MAX_LPI_ID)),
            lpi_to_event: FairSpinlock::new(BTreeMap::new()),
            event_mappings: FairSpinlock::new(BTreeMap::new()),
        }
    }

    fn redistributor_mut(&self) -> &'a mut GicrRegisters {
        let cpu_id = this_cpu!().id;
        // no actual race conditions; relaxed is fine
        let ptr = self.redistributors[cpu_id.to_usize()].load(Ordering::Relaxed);

        debug_assert!(!ptr.is_null());
        unsafe { &mut *ptr }
    }

    fn rdbase_for_redist(&self, index: usize) -> Option<u64> {
        let its = self.its_ref()?;
        let redist_ptr = self.redistributors.get(index)?.load(Ordering::Relaxed);
        let phys_addr = KernelAddressTranslator.dmap_to_phys(redist_ptr as _) as u64;
        let redist_ref = unsafe { &*redist_ptr };

        let proc_num = redist_ref.type_.read_field_pure(GicrTyper::ProcessorNumber) as u64;
        let pta = its.type_.read_field_pure(GitsTyper::PTA);

        Some(if pta { phys_addr >> 16 } else { proc_num })
    }

    fn its_ref(&self) -> Option<&'a GitsRegisters> {
        self.its.as_ref().map(|ptr| {
            let p = ptr.load(Ordering::Relaxed);
            debug_assert!(!p.is_null());
            unsafe { &*p }
        })
    }

    fn wait_for_distributor_rwp(&self) {
        dsb(barrier::ST);

        while self
            .distributor
            .ctl
            .read_field_pure(GicdCtlr::RegisterWritePending)
            == true
        {
            core::hint::spin_loop();
        }
    }

    fn wait_for_redistributor_rwp(&self) {
        dsb(barrier::ISHST);

        let redist = self.redistributor_mut();

        while redist.ctl.read_field_pure(GicrCtlr::RegisterWritePending) == true {
            core::hint::spin_loop();
        }
    }

    fn update_lpi_prop<F>(&self, int_id: u32, update_fn: F) -> Result<()>
    where
        F: FnOnce(&mut RPureReadWrite<u8, LpiProp>),
    {
        if let Some(prop_phys) = *self.lpi_prop_table.borrow() {
            let prop_virt = KernelAddressTranslator.phys_to_dmap(prop_phys as _)
                as *mut RPureReadWrite<u8, LpiProp>;
            let target_ptr = unsafe { prop_virt.add(int_id as usize) };
            unsafe {
                update_fn(&mut *target_ptr);
                clean_dcache_range(target_ptr as _, 1);
            }
            dsb(barrier::ISH);

            if self.its.is_some() {
                let mapping = self.lpi_to_event.lock().get(&int_id).copied();
                if let Some((device_id, event_id, icid)) = mapping {
                    self.push_inv(device_id, event_id)?;

                    if let Some(rdbase) = self.rdbase_for_redist(icid as usize) {
                        self.push_sync(rdbase)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn push_cmd<F>(&self, build_cmd: F) -> Result<()>
    where
        F: FnOnce(&mut [RPureReadWrite<u64, GicBitfield64>; 4]),
    {
        let its = self.its_ref().ok_or(InterruptError::NotSupported)?;

        let _guard = InterruptGuard::new();
        let mut queue_guard = self.its_cmd_queue.lock();

        let queue = queue_guard.as_mut().ok_or(InterruptError::NotSupported)?;

        let current_creadr = its.creadr.read_field_pure(GitsCreadr::Offset) as usize;

        let next_write_offset = (queue.write_offset + 1) % queue.num_entries;

        if next_write_offset == current_creadr {
            use log::*;
            error!(
                "GICv3: ITS command queue full (CREADR={}, CWRITER={})",
                current_creadr, queue.write_offset
            );
            return Err(InterruptError::CommandQueueFull);
        }

        let offset = queue.write_offset;
        let ptr = unsafe {
            queue.base.as_ptr().add(offset * 32) as *mut [RPureReadWrite<u64, GicBitfield64>; 4]
        };
        let cmd_words = unsafe { &mut *ptr };

        cmd_words[0].write(0);
        cmd_words[1].write(0);
        cmd_words[2].write(0);
        cmd_words[3].write(0);

        build_cmd(cmd_words);

        unsafe { clean_dcache_range(ptr as *const u8, 32) };
        dsb(barrier::ISHST);

        queue.write_offset = next_write_offset;

        its.cwriter
            .modify_field_pure(GitsCwriter::Offset, next_write_offset as u16);

        const MAX_SPINS: u32 = 1_000_000;
        const MAX_RETRIES: u32 = 3;
        let mut retries = 0;

        for _ in 0..MAX_SPINS {
            if its.creadr.read_field_pure(GitsCreadr::Offset) as usize == next_write_offset {
                return Ok(());
            }

            if its.creadr.read_field_pure(GitsCreadr::Stalled) {
                use log::*;
                if retries < MAX_RETRIES {
                    warn!(
                        "GICv3: ITS stalled on command; retrying ({}/{MAX_RETRIES})",
                        retries + 1
                    );
                    its.cwriter.modify_field_pure(GitsCwriter::Retry, true);
                    retries += 1;
                } else {
                    error!("GICv3: ITS permanently stalled; aborting command");

                    return Err(InterruptError::HardwareStalled);
                }
            }

            core::hint::spin_loop();
        }

        log::error!(
            "GICv3: ITS timeout waiting for CREADR to advance to offset {}",
            next_write_offset
        );
        Err(InterruptError::CommandTimeout)
    }

    fn push_mapd(
        &self,
        device_id: u32,
        itt_addr: u64,
        num_events_log2: u32,
        valid: bool,
    ) -> Result<()> {
        let size = if num_events_log2 > 0 {
            num_events_log2 - 1
        } else {
            0
        };

        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };
            let w1 = unsafe {
                &mut *(words.as_mut_ptr().add(1) as *mut RPureReadWrite<u64, ItsMapdWord1>)
            };
            let w2 = unsafe {
                &mut *(words.as_mut_ptr().add(2) as *mut RPureReadWrite<u64, ItsMapdWord2>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x08);
            w0.modify_field(ItsCmdWord0::DeviceId, device_id);
            w1.modify_field(ItsMapdWord1::Size, size as u8);
            w2.modify_field(ItsMapdWord2::IttAddr, itt_addr >> 8);
            w2.modify_field(ItsMapdWord2::Valid, valid);
        })
    }

    fn push_mapc(&self, icid: u32, rdbase_val: u64, valid: bool) -> Result<()> {
        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };
            let w2 = unsafe {
                &mut *(words.as_mut_ptr().add(2) as *mut RPureReadWrite<u64, ItsMapcWord2>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x09);
            w2.modify_field(ItsMapcWord2::Icid, icid as u16);
            w2.modify_field(ItsMapcWord2::RdBase, rdbase_val);
            w2.modify_field(ItsMapcWord2::Valid, valid);
        })
    }

    fn push_mapti(&self, device_id: u32, event_id: u32, lpi_id: u32, icid: u32) -> Result<()> {
        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };
            let w1 = unsafe {
                &mut *(words.as_mut_ptr().add(1) as *mut RPureReadWrite<u64, ItsMaptiWord1>)
            };
            let w2 = unsafe {
                &mut *(words.as_mut_ptr().add(2) as *mut RPureReadWrite<u64, ItsMaptiWord2>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x0A);
            w0.modify_field(ItsCmdWord0::DeviceId, device_id);
            w1.modify_field(ItsMaptiWord1::EventId, event_id);
            w1.modify_field(ItsMaptiWord1::pIntId, lpi_id);
            w2.modify_field(ItsMaptiWord2::Icid, icid as u16);
        })
    }

    fn push_invall(&self, icid: u32) -> Result<()> {
        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };
            let w2 = unsafe {
                &mut *(words.as_mut_ptr().add(2) as *mut RPureReadWrite<u64, ItsInvallWord2>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x0D);
            w2.modify_field(ItsInvallWord2::Icid, icid as u16);
        })
    }

    fn push_sync(&self, rdbase_val: u64) -> Result<()> {
        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };
            let w2 = unsafe {
                &mut *(words.as_mut_ptr().add(2) as *mut RPureReadWrite<u64, ItsSyncWord2>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x05);
            w2.modify_field(ItsSyncWord2::RdBase, rdbase_val);
        })
    }

    fn push_discard(&self, device_id: u32, event_id: u32) -> Result<()> {
        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };
            let w1 = unsafe {
                &mut *(words.as_mut_ptr().add(1) as *mut RPureReadWrite<u64, ItsDiscardWord1>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x0F);
            w0.modify_field(ItsCmdWord0::DeviceId, device_id);
            w1.modify_field(ItsDiscardWord1::EventId, event_id);
        })
    }

    fn push_inv(&self, device_id: u32, event_id: u32) -> Result<()> {
        self.push_cmd(|words| {
            let w0 = unsafe {
                &mut *(words.as_mut_ptr().add(0) as *mut RPureReadWrite<u64, ItsCmdWord0>)
            };

            let w1 = unsafe {
                &mut *(words.as_mut_ptr().add(1) as *mut RPureReadWrite<u64, ItsInvWord1>)
            };

            w0.modify_field(ItsCmdWord0::Cmd, 0x0C);
            w0.modify_field(ItsCmdWord0::DeviceId, device_id);
            w1.modify_field(ItsInvWord1::EventId, event_id);
        })
    }

    fn init_its(&self) {
        if let Some(its) = self.its_ref() {
            its.ctl.modify_field_pure(GitsCtlr::Enabled, false);
            while its.ctl.read_field_pure(GitsCtlr::Quiescent) == false {
                core::hint::spin_loop();
            }

            for baser in &its.baser {
                let ty = baser.read_field_pure(GitsBaser::Type);
                if ty == 1 || ty == 4 {
                    let pages = if ty == 4 { 1 } else { 16 };
                    let layout =
                        core::alloc::Layout::from_size_align(pages * ITS_PAGE_SIZE, ITS_PAGE_ALIGN)
                            .expect("invalid size/alignment passed to `from_size_align`");

                    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
                    assert!(!ptr.is_null());
                    let phys = KernelAddressTranslator.dmap_to_phys(ptr as _) as u64;

                    unsafe { clean_dcache_range(ptr, pages * ITS_PAGE_SIZE) };
                    dsb(barrier::ISH);

                    let builder = baser
                        .builder_pure()
                        .set(GitsBaser::PageSize, 0)
                        .set(GitsBaser::Shareability, 1)
                        .set(GitsBaser::InnerCacheability, ITS_CACHEABILITY)
                        .set(GitsBaser::OuterCacheability, ITS_CACHEABILITY)
                        .set(GitsBaser::PhysicalAddress, phys >> ITS_PAGE_SHIFT)
                        .set(GitsBaser::Size, (pages - 1) as u8)
                        .set(GitsBaser::Valid, true);

                    baser.write_pure(builder.finish());
                }
            }

            let cmd_pages = 16;
            let cmd_layout =
                core::alloc::Layout::from_size_align(cmd_pages * ITS_PAGE_SIZE, ITS_PAGE_ALIGN)
                    .expect("invalid size/alignment passed to `from_size_align`");
            let cmd_ptr = unsafe { alloc::alloc::alloc_zeroed(cmd_layout) };
            assert!(!cmd_ptr.is_null());
            let cmd_phys = KernelAddressTranslator.dmap_to_phys(cmd_ptr as _) as u64;

            unsafe { clean_dcache_range(cmd_ptr, cmd_pages * ITS_PAGE_SIZE) };
            dsb(barrier::ISH);

            let builder = its
                .cbaser
                .builder_pure()
                .set(GitsCbasER::Shareability, ITS_SHAREABILITY)
                .set(GitsCbasER::InnerCacheability, ITS_CACHEABILITY)
                .set(GitsCbasER::OuterCacheability, ITS_CACHEABILITY)
                .set(GitsCbasER::PhysicalAddress, cmd_phys >> ITS_PAGE_SHIFT)
                .set(GitsCbasER::Size, (cmd_pages - 1) as u8)
                .set(GitsCbasER::Valid, true);

            its.cbaser.write_pure(builder.finish());

            *self.its_cmd_queue.lock() = Some(ItsCmdQueue {
                base: NonNull::new(cmd_ptr).unwrap(),
                write_offset: 0,
                num_entries: cmd_pages * ITS_PAGE_SIZE / 32,
            });

            its.ctl.modify_field_pure(GitsCtlr::Enabled, true);
        }
    }
}

impl<'a, I: InterruptInterface + Send + Sync> InterruptController for GicV3<'a, I> {
    fn init(&self) -> Result<()> {
        ICC_SRE_EL1.modify(ICC_SRE_EL1::SRE::Enabled);
        {
            let value = 0;
            unsafe { asm!("msr icc_bpr1_el1, {0:x}", in(reg) value) };
        }
        self.iface.enable_group1();
        self.iface.set_priority_mask(0xFF); // unmask every level
        isb(barrier::SY);

        match INIT_STATE.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => {
                self.distributor
                    .ctl
                    .modify_field_pure(GicdCtlr::EnableGroup1, false);

                self.wait_for_distributor_rwp();

                self.distributor.ctl.modify_field_pure(GicdCtlr::Are, true);

                self.wait_for_distributor_rwp();

                // shared peripheral interrupts
                for i in 1..32 {
                    self.distributor.iclear_enable[i].write_pure(0xFFFF_FFFF); // disable
                    self.distributor.iclear_pend[i].write_pure(0xFFFF_FFFF); // clear pending
                    self.distributor.igroup[i].write_pure(0xFFFF_FFFF); // group 1 (non-secure)
                }

                for i in 32..1020 {
                    self.distributor.ipriority[i].write_pure(0xA0); // default priority
                }

                if self.its.is_some() {
                    let prop_pages = 16;
                    let prop_layout = core::alloc::Layout::from_size_align(
                        prop_pages * ITS_PAGE_SIZE,
                        ITS_PAGE_ALIGN,
                    )
                    .expect("invalid size/align passed to `from_size_align`");
                    let prop_ptr = unsafe { alloc::alloc::alloc_zeroed(prop_layout) };
                    let prop_phys = KernelAddressTranslator.dmap_to_phys(prop_ptr as _) as u64;
                    *self.lpi_prop_table.borrow_mut() = Some(prop_phys);

                    unsafe {
                        core::ptr::write_bytes(prop_ptr, 0xA0, prop_pages * ITS_PAGE_SIZE);
                        clean_dcache_range(prop_ptr, prop_pages * ITS_PAGE_SIZE);
                    };
                    dsb(barrier::ISH);

                    self.init_its();
                }

                self.distributor
                    .ctl
                    .modify_field_pure(GicdCtlr::EnableGroup1, true);

                self.wait_for_distributor_rwp();

                INIT_STATE.store(2, Ordering::Release);
                sev();
            }
            Err(_) => {
                while INIT_STATE.load(Ordering::Acquire) != 2 {
                    wfe();
                }
            }
        }

        let redist = self.redistributor_mut();

        redist.wake.modify_field(GicrWaker::ProcessorSleep, false);
        dsb(barrier::SY);

        while redist.wake.read_field_pure(GicrWaker::ProcessorSleep) == true {
            core::hint::spin_loop();
        }

        while redist.wake.read_field_pure(GicrWaker::ChildrenAsleep) == true {
            core::hint::spin_loop();
        }

        // software generated interrupts and private peripheral interrupts
        redist.iclear_enable0.write(0xFFFF_FFFF); // disable SGI/PPI
        redist.iclear_pend0.write(0xFFFF_FFFF); // clear pending
        redist.igroup0.write(0xFFFF_FFFF); // group 1 (non-secure)
        redist.igroup_mod.write(0xFFFF_FFFF);

        if self.its.is_some() {
            let pend_layout =
                core::alloc::Layout::from_size_align(16 * ITS_PAGE_SIZE, ITS_PAGE_ALIGN)
                    .expect("invalid size/align passed to `from_size_align`");
            let pend_ptr = unsafe { alloc::alloc::alloc_zeroed(pend_layout) };
            let pend_phys = KernelAddressTranslator.dmap_to_phys(pend_ptr as _) as u64;

            unsafe { clean_dcache_range(pend_ptr, 16 * ITS_PAGE_SIZE) };
            dsb(barrier::ISH);

            let prop_phys = self.lpi_prop_table.borrow().unwrap();

            let builder = redist
                .property_bar
                .builder_impure()
                .set(GicrPropBar::PhysicalAddress, prop_phys >> ITS_PAGE_SHIFT)
                .set(GicrPropBar::IDBits, 14)
                .set(GicrPropBar::Shareability, ITS_SHAREABILITY)
                .set(GicrPropBar::InnerCacheability, ITS_CACHEABILITY)
                .set(GicrPropBar::OuterCacheability, ITS_CACHEABILITY);

            redist.property_bar.write(builder.finish());

            let cas: u64 = ((ITS_SHAREABILITY as u64) << 10) | ((ITS_CACHEABILITY as u64) << 7);
            redist.pending_bar.write(pend_phys | cas);

            redist.ctl.modify_field(GicrCtlr::EnableLPIs, true);

            if self.its.is_some() {
                let cpu_id = this_cpu!().id;
                let icid = cpu_id.to_u32();

                if let Some(rdbase) = self.rdbase_for_redist(icid as usize) {
                    self.push_mapc(icid, rdbase, true)?;
                    self.push_invall(icid)?;
                    self.push_sync(rdbase)?;
                }
            }
        }

        self.wait_for_redistributor_rwp();

        for i in 0..32 {
            redist.ipriority[i].write(0xA0); // default priority
        }

        isb(barrier::SY);

        Ok(())
    }

    fn enable_interrupt(&self, int_id: u32) -> Result<()> {
        if int_id < 32 {
            let redist = self.redistributor_mut();
            redist.iset_enable0.write(1 << int_id);
            self.wait_for_redistributor_rwp();
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.iset_enable[reg_i].write_pure(1 << bit);
            self.wait_for_distributor_rwp();
        } else if (LPI_START..=MAX_LPI_ID).contains(&int_id) {
            self.update_lpi_prop(int_id, |prop| {
                prop.modify_field(LpiProp::Enabled, true);
            })?;
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }
        Ok(())
    }

    fn disable_interrupt(&self, int_id: u32) -> Result<()> {
        if int_id < 32 {
            let redist = self.redistributor_mut();
            redist.iclear_enable0.write(1 << int_id);
            self.wait_for_redistributor_rwp();
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.iclear_enable[reg_i].write_pure(1 << bit);
            self.wait_for_distributor_rwp();
        } else if (LPI_START..=MAX_LPI_ID).contains(&int_id) {
            self.update_lpi_prop(int_id, |prop| {
                prop.modify_field(LpiProp::Enabled, false);
            })?;
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }
        Ok(())
    }

    fn acknowledge_interrupt(&self) -> Result<Option<u32>> {
        let int_id = self.iface.read_iar();

        // id 1023 is defined as spurious
        if int_id == 1023 {
            Ok(None)
        } else {
            Ok(Some(int_id))
        }
    }

    fn end_of_interrupt(&self, int_id: u32) -> Result<()> {
        if int_id < 1020 || (LPI_START..=MAX_LPI_ID).contains(&int_id) {
            self.iface.write_eoir(int_id);
            Ok(())
        } else {
            Err(InterruptError::InvalidInterruptId)
        }
    }

    fn set_priority(&self, int_id: u32, priority: u8) -> Result<()> {
        if int_id < 32 {
            let redist = self.redistributor_mut();

            redist.ipriority[int_id as usize].write(priority);
        } else if int_id < 1020 {
            self.distributor.ipriority[int_id as usize].write_pure(priority);
        } else if (LPI_START..=MAX_LPI_ID).contains(&int_id) {
            self.update_lpi_prop(int_id, |prop| {
                prop.modify_field(LpiProp::Priority, priority >> 2);
            })?;
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }
        Ok(())
    }

    fn set_affinity(&self, int_id: u32, affinity: u64) -> Result<()> {
        if int_id < 32 {
            // SGIs and PPIs are private to a core
            return Err(InterruptError::NotSupported);
        } else if int_id >= 1020 {
            return Err(InterruptError::InvalidInterruptId);
        }

        self.distributor.irouter[int_id as usize].write_pure(affinity);
        Ok(())
    }

    fn register_handler(&self, int_id: u32, handler: IrqHandler) -> Result<()> {
        if int_id < 1020 {
            let mut handle = self
                .interrupt_handlers
                .try_borrow_mut()
                .map_err(|_| InterruptError::NotSupported)?;
            handle[int_id as usize] = Some(handler);
        } else if (LPI_START..=MAX_LPI_ID).contains(&int_id) {
            let mut handle = self.lpi_handlers.write();
            handle.insert(int_id, handler);
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }

        Ok(())
    }

    fn on_interrupt(&self, int_id: u32) -> Result<()> {
        if int_id < 1020 {
            // if the handle is already being borrowed mutably, that's a bigger problem. panic.
            let handle = self.interrupt_handlers.borrow();

            let handler_fn = handle[int_id as usize]
                .map_or(Err(InterruptError::HandlerNotFound), |h| Ok(h.dispatch_fn))?
                .to_fn();

            handler_fn(int_id)
        } else if (LPI_START..=MAX_LPI_ID).contains(&int_id) {
            // if the handle is already being borrowed mutably, that's a bigger problem. panic.
            let handler_fn = {
                let handle = self.lpi_handlers.read();
                handle
                    .get(&int_id)
                    .map_or(Err(InterruptError::HandlerNotFound), |h| Ok(h.dispatch_fn))?
                    .to_fn()
            };

            handler_fn(int_id)
        } else {
            Err(InterruptError::InvalidInterruptId)
        }
    }

    fn msi_register_device(&self, device_id: u32, num_events_log2: u32) -> Result<()> {
        if self.its.is_none() {
            return Err(InterruptError::MsiNotSupported);
        }

        let num_events = 2usize.pow(num_events_log2);
        let itt_entry_size = if let Some(its) = self.its_ref() {
            its.type_.read_field_pure(GitsTyper::ITTEntrySize) as usize + 1
        } else {
            8
        };

        let itt_size = (num_events * itt_entry_size).next_multiple_of(256);
        let layout = core::alloc::Layout::from_size_align(itt_size, 256)
            .map_err(|_| InterruptError::OutOfMemory)?;

        let itt_ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
        let itt_non_null = NonNull::new(itt_ptr).ok_or(InterruptError::NotSupported)?;
        let itt_phys = KernelAddressTranslator.dmap_to_phys(itt_ptr as _) as u64;

        unsafe { clean_dcache_range(itt_ptr, itt_size) };
        dsb(barrier::ISH);

        self.push_mapd(device_id, itt_phys, num_events_log2, true)?;

        let mut itts = self.device_itts.borrow_mut();
        if let Some(IttAllocation((old_ptr, old_layout))) =
            itts.insert(device_id, IttAllocation((itt_non_null, layout)))
        {
            unsafe {
                alloc::alloc::dealloc(old_ptr.as_ptr(), old_layout);
            }
        }

        Ok(())
    }

    fn msi_unregister_device(&self, device_id: u32) -> Result<()> {
        if self.its.is_some() {
            self.push_mapd(device_id, 0, 0, false)?;
        }

        let mut itts = self.device_itts.borrow_mut();
        if let Some(IttAllocation((ptr, layout))) = itts.remove(&device_id) {
            unsafe { alloc::alloc::dealloc(ptr.as_ptr(), layout) };
        }

        Ok(())
    }

    fn msi_map_event(
        &self,
        device_id: u32,
        event_id: u32,
        lpi_id: Option<u32>,
        cpu: CpuIdLogical,
    ) -> Result<u32> {
        let mut alloc = self.lpi_alloc.lock();

        let lpi = match lpi_id {
            Some(id) => {
                if alloc.reserve(id).is_err() {
                    return Err(InterruptError::LpiAlreadyUsed);
                }
                id
            }
            None => alloc.alloc().ok_or(InterruptError::NoAvailableLpi)?,
        };

        let icid = cpu.to_u32();

        if self.its.is_some() {
            self.push_mapti(device_id, event_id, lpi, icid)?;
            self.event_mappings
                .lock()
                .insert((device_id, event_id), lpi);

            self.lpi_to_event
                .lock()
                .insert(lpi, (device_id, event_id, icid));
        } else {
            alloc.free(lpi);
            return Err(InterruptError::MsiNotSupported);
        }

        Ok(lpi)
    }

    fn msi_unmap_event(&self, device_id: u32, event_id: u32) -> Result<()> {
        if self.its.is_none() {
            return Ok(());
        }

        self.push_discard(device_id, event_id)?;

        for i in 0..self.redistributors.len() {
            if let Some(rdbase) = self.rdbase_for_redist(i) {
                self.push_sync(rdbase)?;
            }
        }

        let lpi = self.event_mappings.lock().remove(&(device_id, event_id));
        if let Some(lpi) = lpi {
            self.lpi_to_event.lock().remove(&lpi);
            self.lpi_alloc.lock().free(lpi);
            self.lpi_handlers.write().remove(&lpi);
        }

        Ok(())
    }

    fn msi_invall(&self, cpu: CpuIdLogical) -> Result<()> {
        let icid = cpu.to_u32();

        if self.its.is_some() {
            self.push_invall(icid)?;
            if let Some(rdbase) = self.rdbase_for_redist(icid as usize) {
                self.push_sync(rdbase)?;
            }
        }

        Ok(())
    }

    fn msi_get_doorbell(&self) -> Result<u64> {
        let its = self.its_ref().ok_or(InterruptError::MsiNotSupported)?;

        let ptr = &its.translator as *const _;

        Ok(KernelAddressTranslator.dmap_to_phys(ptr as _) as u64)
    }
}
