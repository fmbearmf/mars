#![no_std]
#![no_main]

mod allocator;
mod earlyinit;

extern crate alloc;

use aarch64_cpu::{
    asm::{
        barrier::{self, isb},
        wfe,
    },
    registers::{
        CPACR_EL1, DAIF, MAIR_EL1, MPIDR_EL1, ReadWriteable, Readable, SCTLR_EL1, TCR_EL1,
        TTBR0_EL1, TTBR1_EL1, Writeable,
    },
};
use aarch64_cpu_ext::{
    asm::tlb::{VMALLE1, tlbi},
    structures::tte::{AccessPermission, Shareability},
};
use alloc::{boxed::Box, vec::Vec};
use core::{
    arch::{asm, naked_asm},
    mem::{self, MaybeUninit},
    ops::{Add, Index},
    panic::PanicInfo,
    ptr::{self, NonNull},
    slice::from_raw_parts,
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
        mapper::{free_tables, map_region},
        table_allocator::PMTableAllocator,
    },
    scheduler::Scheduler,
    smccc::cpu_on,
    timer::init_timer,
    vcpu::{CpuDescriptor, add_cpu, vcpu_wait_init, with_cpus, with_this_cpu},
    vec::{DynVec, PMVec, RawVec, StaticVec},
    vm::{
        DMAP_START, MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX, MemoryRegion, MemoryRegionType,
        PAGE_SIZE, TABLE_ENTRIES, TTable, align_down, align_up, dmap_addr_to_phys,
        phys_addr_to_dmap, slab::SlabAllocator,
    },
};
use protocol::BootInfo;
use uefi::{
    boot::{MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::{MemoryMap, MemoryMapMut, MemoryMapOwned},
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

#[global_allocator]
pub static KALLOCATOR: SlabAllocator = SlabAllocator::new();

pub static KPT_ALLOCATOR: KernelPTAllocator = KernelPTAllocator {};

pub static GLOBAL_SCHEDULER: Scheduler<KernelPTAllocator, SlabAllocator> = Scheduler::new();

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

const STACK_SIZE: usize = 128 * 1024;

#[allow(dead_code)]
#[repr(align(16))]
struct KStack([u8; STACK_SIZE]);

#[unsafe(link_section = ".reclaimable.bss")]
static mut KSTACK: KStack = KStack([0u8; STACK_SIZE]);

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(_boot_info_ref: &mut BootInfo) {
    naked_asm!(
        "adrp x9, {stack_base}",
        "add x9, x9, :lo12:{stack_base}",
        //
        "add x9, x9, {stack_size}",
        "and x9, x9, #~0xF",
        "mov sp, x9",
        "b {entry}",
        stack_base = sym KSTACK,
        stack_size = const STACK_SIZE,
        entry = sym kentry,
    );
}

fn kaddr_to_paddr(kernel_load_paddr: usize, kaddr: usize) -> usize {
    (kaddr - unsafe { &__KBASE as *const _ as usize }) + kernel_load_paddr
}

fn kentry(boot_info_ref: MaybeUninit<BootInfo>) -> ! {
    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);

    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);

    unsafe {
        asm!(
            "adr {x}, vector_table_el1",
            "msr vbar_el1, {x}",
            x = out(reg) _,
            options(nomem, nostack),
        );
    }

    let boot_info: BootInfo = unsafe { boot_info_ref.assume_init() };

    {
        let mut lock = EARLYCON.lock();
        *lock = Some(EarlyCon::new(boot_info.serial_uart_address));
    }

    earlycon_writeln!("address of bootinfo: {:#p}", &boot_info);

    let uefi_mmap = &mut unsafe { ptr::read(&boot_info.memory_map) };
    uefi_mmap.sort();
    earlycon_writeln!("uefi_mmap @ {:#x}", uefi_mmap as *const _ as u64);

    let mut acpi_reg_opt: Option<MemoryRegion> = None;

    let mut total = 0usize;
    for entry in uefi_mmap.entries() {
        match entry.ty {
            _ => {
                //earlycon_writeln!(
                //    "MemoryDescriptor {{ phys_start: {:#x}, size: {:#x}, ty: {:?} }}",
                //    entry.phys_start,
                //    entry.page_count as usize * UEFI_PS,
                //    entry.ty
                //);
                total += entry.page_count as usize * UEFI_PS;
            }
        }
        if entry.ty == MemoryType::ACPI_RECLAIM {
            acpi_reg_opt = Some(MemoryRegion {
                base: phys_addr_to_dmap(entry.phys_start) as usize,
                size: (entry.page_count as usize * UEFI_PS),
                region_type: MemoryRegionType::AcpiTables,
            });
        }
    }
    earlycon_writeln!("total: {:#x}", total);

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

    init_mmu();

    let page_allocator = &arm_init(uefi_mmap, boot_info.kernel_regions, boot_info.root_pt);

    // SAFETY: this function never returns, so `page_allocator` can be treated as a static reference.
    let page_alloc_ref: &'static PageAllocator = unsafe { mem::transmute(page_allocator) };

    unsafe { KALLOCATOR.init(page_alloc_ref) };

    let table_allocator = KernelPTAllocator {};

    let xsdt = match rsdp.xsdt(true) {
        Ok(x) => x,
        Err(e) => panic!("xsdt invalid: {}", e),
    };

    earlycon_writeln!("ACPI XSDT @ {:p}", xsdt);

    {
        let iter = XsdtIter::new(xsdt, true);
        for (i, table) in iter.enumerate() {
            let sig = from_utf8(&table.sig).unwrap_or("ERR ");
            let len = unsafe { ptr::read_unaligned(&raw const table.len) };
            earlycon_writeln!("     Table [{}]: \"{}\" ({} bytes)", i, sig, len);
        }
    }

    let sys = SystemDescription::parse(xsdt, true);

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
            let base = unsafe { ptr::read_unaligned(&raw const alloc.base_addr) };
            let seg = unsafe { ptr::read_unaligned(&raw const alloc.pci_segment_group) };
            let start = unsafe { ptr::read_unaligned(&raw const alloc.start_bus_num) };
            let end = unsafe { ptr::read_unaligned(&raw const alloc.end_bus_num) };
            earlycon_writeln!("PCI seg {} Base={:#x}, Bus {}-{}", seg, base, start, end);
        }
    }

    // ditch lower half mappings
    TTBR0_EL1.set(0);
    TCR_EL1.modify(TCR_EL1::EPD0::DisableTTBR0Walks);
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
                &table_allocator,
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
            free_tables(pt_root_ptr, &table_allocator);
        }
    });

    print_mem_usage();

    with_this_cpu(|cpu| {
        assert_eq!(cpu.mpidr, this_mpidr.affinity_only());

        let mut gic = cpu.gic.expect("`None` gic");

        gic.init().expect("gic init fail");
        gic.enable_interrupt(cpu.timer_irq as u32)
            .expect("error enabling timer IRQ");

        DAIF.modify(DAIF::D::Unmasked + DAIF::A::Unmasked + DAIF::I::Unmasked + DAIF::F::Unmasked);

        init_timer();
    });

    earlycon_writeln!("boot core finish");

    busy_loop()
}

fn print_mem_usage() {
    let mut bufs = [[0u8; 16]; 2];
    let bufs_tuple = bufs.split_at_mut(1);

    earlycon_writeln!(
        "heap usage: {} / {}",
        bytes_to_human_readable(KALLOCATOR.heap_usage() as u64, &mut bufs_tuple.0[0]),
        bytes_to_human_readable(KALLOCATOR.heap_capacity() as u64, &mut bufs_tuple.1[0]),
    );
}

pub extern "C" fn arm_init(
    uefi_mmap: &mut MemoryMapOwned,
    memory_regions: StaticVec<MemoryRegion>,
    mut root_pt: NonNull<TTable<TABLE_ENTRIES>>,
) -> PageAllocator {
    let mut pmvec: PMVec<MemoryRegion> = PMVec::new();

    {
        let slice = memory_regions.as_slice();
        pmvec.extend_from_slice(slice);
    }

    for &region in uefi_mmap.entries() {
        let region_type: MemoryRegionType = match region.ty {
            MemoryType::RUNTIME_SERVICES_CODE => MemoryRegionType::RtFirmwareCode,
            MemoryType::RUNTIME_SERVICES_DATA => MemoryRegionType::RtFirmwareData,

            MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE => MemoryRegionType::Mmio,
            MemoryType::CONVENTIONAL | MemoryType::BOOT_SERVICES_DATA => MemoryRegionType::Normal,
            MemoryType::BOOT_SERVICES_CODE => MemoryRegionType::FirmwareReclaim,
            MemoryType::LOADER_CODE => MemoryRegionType::FirmwareReclaim,
            MemoryType::ACPI_RECLAIM => MemoryRegionType::AcpiTables,
            MemoryType::ACPI_NON_VOLATILE => MemoryRegionType::AcpiNvs,
            MemoryType::LOADER_DATA => continue,
            _ => {
                earlycon_writeln!("unknown: {:?}", region);
                MemoryRegionType::Unknown
            }
        };

        let size = if region_type == MemoryRegionType::Mmio {
            align_up(region.page_count as usize * UEFI_PS, PAGE_SIZE)
        } else {
            region.page_count as usize * UEFI_PS
        };

        let prev_opt = pmvec.remove_containing(region.phys_start as usize);
        if let Some(prev) = prev_opt {
            pmvec.push(MemoryRegion {
                base: prev.base.min(region.phys_start as usize),
                size: prev.size.max(size),
                region_type: prev.region_type,
            });
        } else {
            pmvec.push(MemoryRegion {
                base: region.phys_start as usize,
                size,
                region_type,
            });
        }
    }

    let mut pmvec_copy: PMVec<MemoryRegion> = PMVec::new();
    pmvec_copy.extend_from_slice(pmvec.as_slice());
    pmvec_copy.compact();

    for &region in pmvec_copy.as_slice() {
        if region.is_normal() {
            continue;
        }

        _ = pmvec.remove_containing(region.base);
    }
    pmvec.compact();

    let early_page_allocator = PMTableAllocator::new(pmvec);

    earlycon_writeln!("{:#x?}", pmvec_copy.as_slice());
    {
        let mut total = 0usize;
        for &region in pmvec_copy.as_slice() {
            //if !region.is_normal() {
            //    continue;
            //};
            let aligned_base = align_up(region.base, PAGE_SIZE);

            if region.size < PAGE_SIZE {
                continue;
            }

            let mair = if region.region_type == MemoryRegionType::Mmio {
                MAIR_DEVICE_INDEX
            } else {
                MAIR_NORMAL_INDEX
            };

            unsafe {
                map_region(
                    root_pt.as_mut(),
                    aligned_base,
                    phys_addr_to_dmap(aligned_base as u64) as usize,
                    align_down(region.size, PAGE_SIZE),
                    AccessPermission::PrivilegedReadWrite,
                    Shareability::InnerShareable,
                    true,
                    true,
                    mair,
                    &early_page_allocator,
                )
            };
            total += region.size;
        }
        earlycon_writeln!("mem size: {:#x} ({} B)", total, total);
    }

    let mmap_swiss_cheese = early_page_allocator.free_regions.into_inner();

    for &region in mmap_swiss_cheese.as_slice() {
        if let Some(_) = pmvec_copy.remove_containing(region.base) {
            pmvec_copy.push(region);
        }
    }
    pmvec_copy.compact();

    let mut page_allocator = unsafe { PageAllocator::init(&[]) };

    for &region in pmvec_copy.as_slice() {
        if region.is_usable() {
            let aligned_base = align_up(DMAP_START + region.base, PAGE_SIZE);
            let aligned_size = align_down(region.size, PAGE_SIZE);

            if aligned_size <= 2 * PAGE_SIZE {
                continue;
            }

            page_allocator.add_range(MemoryRegion {
                base: aligned_base,
                size: aligned_size,
                region_type: region.region_type,
            });
        }
    }

    let page = page_allocator.alloc_page();
    assert!(!page.is_null());

    earlycon_writeln!(
        "test page start: {:#x}, head: {:#x}",
        page as usize,
        (page as usize).add(PAGE_SIZE)
    );

    page_allocator.free_pages(page);

    page_allocator
}
