#![no_std]
#![no_main]

mod allocator;
mod earlyinit;
mod log;

extern crate alloc;

use ::log::{LevelFilter, debug, info, trace};
use aarch64_cpu::{
    asm::wfe,
    registers::{
        DAIF, MAIR_EL1, MPIDR_EL1, ReadWriteable, Readable, SCTLR_EL1, TCR_EL1, TTBR0_EL1,
        TTBR1_EL1, Writeable,
    },
};
use aarch64_cpu_ext::{
    asm::tlb::{VMALLE1, tlbi},
    structures::tte::{AccessPermission, Shareability},
};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{
    arch::{asm, naked_asm},
    mem::{self, MaybeUninit},
    ops::Index,
    panic::PanicInfo,
    ptr::{self, NonNull},
    range::Range,
    slice::{self, Iter, from_raw_parts},
    str::from_utf8,
};
use klib::{
    acpi::{
        SystemDescription,
        madt::{
            GicCpuInterface, GicDistributor, GicIts, GicRedistributor, GicrFrame, MADT_GICC,
            MADT_GICD, MADT_GICR, MADT_ITS,
        },
        rsdp::{XsdtIter, find_rsdp_in_slice},
    },
    bytes_to_human_readable,
    cpu_interface::{Arm64InterruptInterface, Mpidr, SecondaryBootArgs, mpidr_affinities},
    interrupt::{GicdRegisters, InterruptController, gicv3::GicV3},
    pm::page::{
        PageAllocator,
        mapper::{AddressTranslator, free_tables, map_region},
    },
    process::Process,
    scheduler::Scheduler,
    smccc::cpu_on,
    thread::Thread,
    timer::{init_timer, timer_rearm},
    vcpu::{CpuDescriptor, add_cpu, vcpu_wait_init, with_cpus, with_this_cpu},
    vm::{
        MAIR_NORMAL_INDEX, MemoryRegion, PAGE_SIZE, TABLE_ENTRIES, TTable, align_down, align_up,
        dmap_addr_to_phys,
        page_allocator::PhysicalPageAllocator,
        phys_addr_to_dmap,
        slab::SlabAllocator,
        user::{PAGE_DESCRIPTORS, address_space::AddressSpace},
    },
};
use protocol::BootInfo;
use uefi::{
    boot::MemoryDescriptor,
    mem::memory_map::{MemoryMap, MemoryMapMut},
};

use crate::{
    allocator::KernelAddressTranslator,
    earlyinit::{
        mem::{
            clone_and_process_mmap, create_page_descriptors, early_stack_size_check,
            populate_alloc, print_pt, switch_to_new_page_tables,
        },
        mmu::init_cpu,
    },
    log::LOGGER,
};

use self::{
    allocator::KernelPTAllocator,
    earlyinit::{
        earlycon::{EARLYCON, EarlyCon},
        exception::Exceptions,
        mmu::init_mmu,
        smp::{secondary_entry, secondary_init},
    },
};

klib::exception_handlers!(Exceptions);

// use `KALLOCATOR`
static KPAGE_ALLOCATOR: PageAllocator = PageAllocator::new(&KernelAddressTranslator);

// storage for boot info struct
// shouldn't be accessed outside of very early in kentry
static mut BOOT_INFO: MaybeUninit<BootInfo> = MaybeUninit::uninit();

#[global_allocator]
pub static KALLOCATOR: SlabAllocator =
    SlabAllocator::new(&KPAGE_ALLOCATOR, &KernelAddressTranslator);

pub static KPT_ALLOCATOR: KernelPTAllocator = KernelPTAllocator {};

pub static GLOBAL_SCHEDULER: Scheduler = Scheduler::new();

pub static KERNEL_ADDRESS_SPACE: AddressSpace = unsafe {
    AddressSpace::new_dangling(
        None,
        &KPT_ALLOCATOR,
        &KPAGE_ALLOCATOR,
        &KernelAddressTranslator,
    )
};

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        EARLYCON.force_unlock();
        earlycon_writeln!("{}", info);
    }
    busy_loop();
}

#[allow(dead_code)]
fn busy_loop() -> ! {
    loop {
        wfe();
    }
}

#[allow(dead_code)]
fn busy_loop_ret() {
    loop {
        wfe();
    }
}

unsafe extern "C" {
    pub static __KBASE: usize;
}

const STACK_SIZE: usize = 32 * 1024;

#[allow(dead_code)]
#[repr(align(16))]
struct KStack([u8; STACK_SIZE]);

impl KStack {
    pub const fn new() -> Self {
        Self([0u8; STACK_SIZE])
    }
}

//#[unsafe(link_section = ".reclaimable.bss")]
static mut KSTACK: KStack = KStack::new();

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(_boot_info_ref: *mut BootInfo) {
    naked_asm!(
        "adrp x9, {stack_base}",
        "add x9, x9, :lo12:{stack_base}",
        "add x9, x9, {stack_size}",
        "and x9, x9, #~0xF",
        "mov sp, x9",
        //
        "bl {entry}",
        stack_base = sym KSTACK,
        stack_size = const STACK_SIZE,
        entry = sym kentry,
    );
}

fn kaddr_to_paddr(kernel_load_paddr: usize, kaddr: usize) -> usize {
    (kaddr - unsafe { &__KBASE as *const _ as usize }) + kernel_load_paddr
}

fn kentry(boot_info_ref: *mut BootInfo) -> ! {
    unsafe {
        asm!(
            "adr {x}, vector_table_el1",
            "msr vbar_el1, {x}",
            x = out(reg) _,
            options(nomem, nostack),
        );
    }
    init_cpu();

    #[allow(static_mut_refs, reason = "singlethreaded access")]
    {
        unsafe {
            ptr::copy_nonoverlapping(boot_info_ref, BOOT_INFO.as_mut_ptr(), 1);
        };
    }

    #[allow(static_mut_refs, reason = "singlethreaded access")]
    let boot_info = unsafe { BOOT_INFO.assume_init_mut() };

    {
        let mut lock = EARLYCON.lock();
        *lock = Some(EarlyCon::new(boot_info.serial_uart_address));
    }

    LOGGER
        .init(LevelFilter::Trace)
        .expect("failed to init logger");

    info!("plug");

    trace!("address of passed bootinfo ptr: {:#p}", boot_info_ref);
    trace!("address of bootinfo: {:#p}", &boot_info);

    trace!("init_mmu addr: {:#p}", init_mmu as *const ());
    init_mmu(boot_info.page_table_root);

    let uefi_mmap = &mut boot_info.memory_map;
    uefi_mmap.sort();

    trace!("uefi_mmap @ {:p}", uefi_mmap.buffer() as *const _);

    let uefi_mmap = clone_and_process_mmap(uefi_mmap);
    trace!("processed uefi_mmap @ {:p}", uefi_mmap.buffer() as *const _);

    for desc in uefi_mmap.entries() {
        trace!("{:x?}", desc);
    }

    populate_alloc(&uefi_mmap);

    let mut pt_root = unsafe { switch_to_new_page_tables(|| uefi_mmap.entries(), &KALLOCATOR) };

    //print_pt(unsafe { pt_root.as_mut() }, false);

    unsafe { KALLOCATOR.transition_dmap() };

    let (page_descriptors, range) = create_page_descriptors();
    PAGE_DESCRIPTORS.init(page_descriptors, range);

    unsafe { KERNEL_ADDRESS_SPACE.init() };

    {
        let mut lock = EARLYCON.lock();
        if let Some(uart) = &mut *lock {
            // TODO: correctly map the rest of MMIO into DMAP
            //uart.switch(KernelAddressTranslator.phys_to_dmap(boot_info.serial_uart_address) as _);
        }
    }

    debug!("weldington");

    busy_loop();

    let mut acpi_reg_opt: Option<MemoryRegion> = None;

    let acpi_reg = acpi_reg_opt.expect("no ACPI tables found.");
    let slice = unsafe {
        from_raw_parts(
            dmap_addr_to_phys(acpi_reg.base as u64) as *const u8,
            acpi_reg.size,
        )
    };

    // scanning for the RSDP could be avoided by passing a pointer from UEFI via the boot protocol,
    // but that creates dependence on ACPI (which i'd really rather avoid).
    let rsdp = find_rsdp_in_slice(slice).expect("RSDP not found in ACPI tables.");
    earlycon_writeln!("ACPI RSDP @ {:p}", rsdp);

    let xsdt = match rsdp.xsdt() {
        Ok(x) => x,
        Err(e) => panic!("xsdt invalid: {}", e),
    };

    earlycon_writeln!("ACPI XSDT @ {:p}", xsdt);

    {
        let iter = XsdtIter::new(xsdt);
        for (i, table) in iter.enumerate() {
            let sig = from_utf8(&table.sig).unwrap_or("ERR ");
            let len = table.len();
            earlycon_writeln!("     Table [{}]: \"{}\" ({} bytes)", i, sig, len);
        }
    }

    let sys = SystemDescription::parse(xsdt);

    if let Some(fadt) = sys.fadt {
        earlycon_writeln!("FADT found. X_DSDT: {:#x}", sys.dsdt_addr);
        let arm_flags = fadt.arm_boot_arch();
        earlycon_writeln!("arm boot arch: {:#06x}", arm_flags);
        if (arm_flags & 1) != 0 {
            earlycon_writeln!("PSCI compliant");
        }
    }

    let mut cpu_gicrs: Vec<GicrFrame> = Vec::new();
    let mut cpu_gicd: Option<*const GicdRegisters> = None;

    let mut timer_irq = None;

    if let Some(gtdt) = sys.gtdt {
        let virt = gtdt.virt_el1_gsiv();
        let phys = gtdt.ns_el1_gsiv();
        earlycon_writeln!("GTDT timer GSIVs Virt={}, Phys={}", virt, phys);
        timer_irq = Some(virt);
    }

    if let Some(madt) = sys.madt {
        earlycon_writeln!("MADT found");
        let mut cpu_count = 0u32;

        for (type_, data) in madt.entries() {
            match type_ {
                MADT_GICC => {
                    if data.len() >= mem::size_of::<GicCpuInterface>() {
                        let gicc = unsafe { &*(data.as_ptr() as *const GicCpuInterface) };

                        cpu_count += 1;

                        let mpidr = gicc.mpidr();
                        earlycon_writeln!("     GICC CPU[{}] MPIDR={:#x}", cpu_count - 1, mpidr);
                    }
                }
                _ => {}
            }
        }

        for (type_, data) in madt.entries() {
            match type_ {
                MADT_GICD => {
                    if data.len() >= mem::size_of::<GicDistributor>() {
                        let gicd = unsafe { &*(data.as_ptr() as *const GicDistributor) };
                        let id = gicd.gic_id();
                        let base = gicd.phys_base();
                        earlycon_writeln!("     GICD Distributor ID={} Base={:#x}", id, base);

                        let gicd_ptr = base as *const GicdRegisters;
                        cpu_gicd = Some(phys_addr_to_dmap(gicd_ptr as u64) as *const _);
                    }
                }
                MADT_GICR => {
                    if data.len() >= mem::size_of::<GicRedistributor>() {
                        let gicr = unsafe { &*(data.as_ptr() as *const GicRedistributor) };
                        let base = gicr.discovery_range_base();
                        let size = gicr.discovery_range_len();
                        earlycon_writeln!(
                            "     GICR Redistributors Base={:#x} Size={:#x}",
                            base,
                            size
                        );

                        let gicr_frames = unsafe { gicr.frames(true).expect("gicr split error") };

                        for i in 0..gicr_frames.len().min(cpu_count as usize) {
                            let frame = gicr_frames.get(i, true).unwrap();

                            let rd = frame.rd;
                            let sgi = frame.sgi;

                            let type_r = rd.TYPER.get();

                            earlycon_writeln!(
                                "frame {}: sgi@{:p}, rd@{:p} type_r: {:064b}",
                                i,
                                sgi,
                                rd,
                                type_r
                            );

                            cpu_gicrs.push(frame);
                        }
                    }
                }
                MADT_ITS => {
                    if data.len() >= mem::size_of::<GicIts>() {
                        let its = unsafe { &*(data.as_ptr() as *const GicIts) };
                        let id = its.translation_id();
                        let base = its.phys_base();

                        earlycon_writeln!("     ITS ID={}, Base={:#x}", id, base);
                    }
                }
                _ => {}
            }
        }

        for (type_, data) in madt.entries() {
            match type_ {
                MADT_GICC => {
                    if data.len() >= mem::size_of::<GicCpuInterface>() {
                        let gicc = unsafe { &*(data.as_ptr() as *const GicCpuInterface) };
                        let flags = gicc.flags();

                        let enabled = (flags & 0x1) != 0;
                        let online_capable = (flags & 0x8) != 0;

                        let mpidr_tuple = mpidr_affinities(gicc.mpidr());
                        let mpidr =
                            Mpidr::new(mpidr_tuple.0, mpidr_tuple.1, mpidr_tuple.2, mpidr_tuple.3);

                        let gicr = cpu_gicrs.index(mpidr.affinity_only() as usize);
                        let gicd_ptr = cpu_gicd.expect("`None` gicd");
                        let gicd = unsafe { &*gicd_ptr };
                        let gic = GicV3::new(gicd, gicr.rd, gicr.sgi, Arm64InterruptInterface {});

                        add_cpu(
                            CpuDescriptor {
                                acpi_cpu_uid: gicc.acpi_cpu_uid(),
                                mpidr: mpidr.affinity_only(),
                                available: enabled || online_capable,
                                efficiency_class: gicc.efficiency_class(),
                                gic: Some(gic),
                                timer_irq: timer_irq.expect("timer_irq not set") as u64,
                            },
                            &GLOBAL_SCHEDULER,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(mcfg) = sys.mcfg {
        earlycon_writeln!("MCFG found.");
        for alloc in mcfg.allocations() {
            let base = alloc.base_addr();
            let seg = alloc.pci_segment_group();
            let start = alloc.start_bus_num();
            let end = alloc.end_bus_num();
            earlycon_writeln!("PCI seg {} Base={:#x}, Bus {}-{}", seg, base, start, end);
        }
    }

    // ditch lower half mappings
    TTBR0_EL1.set(0);
    //TCR_EL1.modify(TCR_EL1::EPD0::DisableTTBR0Walks);
    tlbi(VMALLE1);

    let this_mpidr = Mpidr::current();
    let is_uniprocessor = MPIDR_EL1::U.read(this_mpidr.affinity_only()) == 1;
    earlycon_writeln!("Uniprocessor?: {}", is_uniprocessor);

    if is_uniprocessor {
        unimplemented!();
    }

    print_mem_usage();

    with_cpus(|_, cpus| {
        for cpu in cpus {
            if cpu.mpidr == this_mpidr.affinity_only() {
                continue;
            }

            let mpidr_tuple = mpidr_affinities(cpu.mpidr);
            let mpidr = Mpidr::new(mpidr_tuple.0, mpidr_tuple.1, mpidr_tuple.2, mpidr_tuple.3);

            let stack = Box::new([0u8; 16384]);
            let stack_ptr = Box::into_raw(stack);

            // add(1) means add 16384 bytes because the size of `stack` is 16384
            // i was VERY confused before i realized that...

            let stack_ptr_aligned = align_down(unsafe { stack_ptr.add(1) } as usize, 16);

            let page_tables = Box::new(TTable::<TABLE_ENTRIES>::new());
            let page_tables_ref = unsafe { &mut *Box::into_raw(page_tables) };
            let paddr = align_down(boot_info.kernel_load_physical_address, PAGE_SIZE);
            let size = align_up(boot_info.kernel_size, PAGE_SIZE);

            earlycon_writeln!(
                "root: {:p}, paddr: {:#x}, size: {:#x}",
                page_tables_ref,
                paddr,
                size
            );
            map_region(
                page_tables_ref,
                paddr,
                paddr,
                size,
                AccessPermission::PrivilegedReadWrite,
                Shareability::InnerShareable,
                true,
                false,
                MAIR_NORMAL_INDEX,
                &KPT_ALLOCATOR,
                &KernelAddressTranslator,
            );

            let secondary_tcr = TCR_EL1::TBI1::Ignored
                + TCR_EL1::IPS::Bits_48
                + TCR_EL1::TG1::KiB_16
                + TCR_EL1::SH1::Inner
                + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::EPD1::EnableTTBR1Walks
                + TCR_EL1::T1SZ.val(16)
                + TCR_EL1::TBI0::Ignored
                + TCR_EL1::TG0::KiB_16
                + TCR_EL1::SH1::Inner
                + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
                + TCR_EL1::EPD0::EnableTTBR0Walks
                + TCR_EL1::T0SZ.val(16);

            let args = Box::new(SecondaryBootArgs {
                ttbr0: dmap_addr_to_phys(page_tables_ref as *const _ as u64),
                ttbr1: TTBR1_EL1.get(),
                tcr: secondary_tcr.value,
                mair: MAIR_EL1.get(),
                stack_top_virt: stack_ptr_aligned as u64,
                entry_virt: secondary_init as *const () as u64,
                sctlr: SCTLR_EL1.get(),
                cpu_desc: cpu as *const _,
            });

            let args_ptr = Box::into_raw(args);

            let entry = kaddr_to_paddr(
                boot_info.kernel_load_physical_address,
                secondary_entry as *const () as usize,
            );

            earlycon_writeln!("secondary entry @ {:#x}", entry);

            let result = cpu_on(
                true,
                mpidr,
                entry as u64,
                dmap_addr_to_phys(args_ptr as u64),
            );

            result.expect("secondary cpu enable fail");

            vcpu_wait_init(mpidr.affinity_only() as usize);

            let pt_root_ptr = unsafe { NonNull::new_unchecked(page_tables_ref as *mut _) };

            drop(unsafe { Box::from_raw(args_ptr) });
            drop(unsafe { Box::from_raw(stack_ptr) });
            free_tables(pt_root_ptr, &KPT_ALLOCATOR, &KernelAddressTranslator);
        }
    });

    let process = {
        let proc = Arc::new(Process::new(
            1,
            AddressSpace::new(None, &KPT_ALLOCATOR, &KALLOCATOR, &KernelAddressTranslator),
            None,
        ));

        proc.with_address_space(|address_space| {
            let mut curr_sorr = address_space.lock(Range {
                start: 0x0,
                end: 0x4000,
            });

            let page_pa: usize = KALLOCATOR.alloc_phys_page().expect("where my page at");
            let page_ptr: &mut [u32] = unsafe {
                slice::from_raw_parts_mut(
                    KernelAddressTranslator.phys_to_dmap(page_pa as _) as *mut u32,
                    PAGE_SIZE,
                )
            };

            let code: &[u32] = &[
                0xD503_201F, // NOP
                0xD280_0000, // MOV x0, #0
                0x9100_03E1, // MOV x1, sp
                // loop:
                0x9100_0400, // ADD x0, x0, #1
                0xD100_07FF, // SUB sp, sp, #1
                0xF100_101F, // CMP x0, #4
                0x54FF_FFAB, // B.LT loop
                // regular:
                0xD503_203F, // YIELD
                0x17FF_FFFF, // B regular
            ];

            for (d, s) in page_ptr.iter_mut().zip(code.iter()) {
                *d = *s;
            }

            curr_sorr.map(
                page_pa as u64,
                AccessPermission::ReadOnly,
                Shareability::InnerShareable,
                false,
                true,
                MAIR_NORMAL_INDEX,
            );
        });

        proc
    };

    let thread = {
        let stack: Box<[u8]> = Box::new([0u8; 4096]);

        process.with_address_space(|address_space| {
            let range_va = stack.as_ptr_range();
            let range_va = Range {
                start: range_va.start as usize,
                end: range_va.end as usize,
            };

            let range_pa = Range {
                start: KernelAddressTranslator.dmap_to_phys(range_va.start as *mut u8) as usize,
                end: KernelAddressTranslator.dmap_to_phys(range_va.end as *mut u8) as usize,
            };

            assert_eq!(range_pa.end as usize & 0x10, 0, "stack unaligned..?");

            let mut curr_sorr = address_space.lock(range_va);

            earlycon_writeln!("    stack map: {:#x?} -> {:#x?}", range_va, range_pa);

            curr_sorr.map(
                range_pa.start as u64,
                AccessPermission::ReadWrite,
                Shareability::InnerShareable,
                true,
                true,
                MAIR_NORMAL_INDEX,
            );
        });

        let thread = Arc::new(Thread::new(
            1,
            &process,
            stack,
            0x0,
            0,
            &KernelAddressTranslator,
        ));

        thread
    };

    process.add_thread(thread.clone());
    GLOBAL_SCHEDULER.spawn(thread);

    earlycon_writeln!("test process: {:?}", process.as_ref());

    print_mem_usage();

    with_this_cpu(|cpu| {
        assert_eq!(cpu.mpidr, this_mpidr.affinity_only());

        let mut gic = cpu.gic.expect("`None` gic");

        gic.init().expect("gic init fail");
        gic.enable_interrupt(cpu.timer_irq as u32)
            .expect("error enabling timer IRQ");

        DAIF.modify(DAIF::D::Unmasked + DAIF::A::Unmasked + DAIF::I::Unmasked + DAIF::F::Unmasked);

        init_timer();
        timer_rearm();
    });

    busy_loop()
}

fn print_mem_usage() {
    let mut bufs = [[0u8; 16]; 2];
    let bufs_tuple = bufs.split_at_mut(1);

    earlycon_writeln!(
        "page usage: {} / {}",
        bytes_to_human_readable(KALLOCATOR.page_usage() as u64, &mut bufs_tuple.0[0]),
        bytes_to_human_readable(KALLOCATOR.capacity() as u64, &mut bufs_tuple.1[0]),
    );
}

pub extern "C" fn alloc_init() -> PageAllocator<'static> {
    let page_allocator = PageAllocator::new(&KernelAddressTranslator);

    page_allocator
}
