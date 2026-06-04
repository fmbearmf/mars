use core::{
    arch::asm,
    fmt::Debug,
    sync::atomic::{AtomicU8, Ordering},
};

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::ReadWriteable as TRW,
};
use atomic_refcell::{AtomicRef, AtomicRefMut};
use mars_models::memory::registers::volatile::{PureReadable, PureWriteable, Writeable};

use crate::{interrupt::GicrRegisters, this_cpu};

use super::{
    GicdRegisters, InterruptController, InterruptError, InterruptInterface,
    gicv3::registers::gic::{GicdCtlr, GicrCtlr, GicrWaker},
};

use self::registers::icc_sre_el1::ICC_SRE_EL1;

pub mod registers;

static INIT_STATE: AtomicU8 = AtomicU8::new(0);

pub struct GicV3<'a, I: InterruptInterface + Send + Sync> {
    pub distributor: &'a GicdRegisters,
    pub iface: I,
}

impl<I: InterruptInterface + Send + Sync> Debug for GicV3<'_, I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GicV3").finish()
    }
}

impl<'a, I: InterruptInterface + Send + Sync> GicV3<'a, I> {
    pub fn new(distributor: &'a mut GicdRegisters, iface: I) -> Self {
        Self { distributor, iface }
    }

    fn redistributor(&self) -> AtomicRef<'_, Option<&'static mut GicrRegisters>> {
        this_cpu!().redistributor.borrow()
    }

    fn redistributor_mut(&self) -> AtomicRefMut<'_, Option<&'static mut GicrRegisters>> {
        this_cpu!().redistributor.borrow_mut()
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

        let guard = self.redistributor();
        let redist = guard.as_ref().expect("`None` redistributor");

        while redist.ctl.read_field_pure(GicrCtlr::RegisterWritePending) == true {
            core::hint::spin_loop();
        }
    }
}

impl<'a, I: InterruptInterface + Send + Sync> InterruptController for GicV3<'a, I> {
    fn init(&self) -> Result<(), InterruptError> {
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

                self.distributor
                    .ctl
                    .modify_field_pure(GicdCtlr::EnableGroup1, true);

                self.wait_for_distributor_rwp();

                INIT_STATE.store(2, Ordering::Release);
            }
            Err(_) => {
                while INIT_STATE.load(Ordering::Acquire) != 2 {
                    core::hint::spin_loop();
                }
            }
        }

        let guard = self.redistributor_mut();
        let mut redist = AtomicRefMut::map(guard, |opt| {
            opt.as_mut().expect("redistributor not initialized")
        });

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

        self.wait_for_redistributor_rwp();

        for i in 0..32 {
            redist.ipriority[i].write(0xA0); // default priority
        }

        isb(barrier::SY);

        Ok(())
    }

    fn enable_interrupt(&self, int_id: u32) -> Result<(), InterruptError> {
        if int_id < 32 {
            let guard = self.redistributor_mut();
            let mut redist = AtomicRefMut::map(guard, |opt| {
                opt.as_mut().expect("redistributor not initialized")
            });

            redist.iset_enable0.write(1 << int_id);
            self.wait_for_redistributor_rwp();
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.iset_enable[reg_i].write_pure(1 << bit);
            self.wait_for_distributor_rwp();
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }
        Ok(())
    }

    fn disable_interrupt(&self, int_id: u32) -> Result<(), InterruptError> {
        if int_id < 32 {
            let guard = self.redistributor_mut();
            let mut redist = AtomicRefMut::map(guard, |opt| {
                opt.as_mut().expect("redistributor not initialized")
            });

            redist.iclear_enable0.write(1 << int_id);
            self.wait_for_redistributor_rwp();
        } else if int_id < 1020 {
            let reg_i = (int_id / 32) as usize;
            let bit = int_id % 32;
            self.distributor.iclear_enable[reg_i].write_pure(1 << bit);
            self.wait_for_distributor_rwp();
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }
        Ok(())
    }

    fn acknowledge_interrupt(&self) -> Result<Option<u32>, InterruptError> {
        let int_id = self.iface.read_iar();

        // id 1023 is defined as spurious
        if int_id == 1023 {
            Ok(None)
        } else {
            Ok(Some(int_id))
        }
    }

    fn end_of_interrupt(&self, int_id: u32) -> Result<(), InterruptError> {
        if int_id < 1020 {
            self.iface.write_eoir(int_id);
            Ok(())
        } else {
            Err(InterruptError::InvalidInterruptId)
        }
    }

    fn set_priority(&self, int_id: u32, priority: u8) -> Result<(), InterruptError> {
        if int_id < 32 {
            let guard = self.redistributor_mut();
            let mut redist = AtomicRefMut::map(guard, |opt| {
                opt.as_mut().expect("redistributor not initialized")
            });

            redist.ipriority[int_id as usize].write(priority as u32);
        } else if int_id < 1020 {
            self.distributor.ipriority[int_id as usize].write_pure(priority as u32);
        } else {
            return Err(InterruptError::InvalidInterruptId);
        }
        Ok(())
    }

    fn set_affinity(&self, int_id: u32, affinity: u64) -> Result<(), InterruptError> {
        if int_id < 32 {
            // SGIs and PPIs are private to a core
            return Err(InterruptError::NotSupported);
        } else if int_id >= 1020 {
            return Err(InterruptError::InvalidInterruptId);
        }

        self.distributor.irouter[int_id as usize].write_pure(affinity);
        Ok(())
    }
}
