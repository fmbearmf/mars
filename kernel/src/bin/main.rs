#![no_std]
#![no_main]
#![feature(fn_traits)]

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
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::boxed::Box;
use core::{
    arch::{asm, naked_asm},
    mem::{self, MaybeUninit},
    ops::Add,
    panic::PanicInfo,
    ptr::{self, NonNull},
    slice::from_raw_parts,
    str::from_utf8,
};
use klib::{
    acpi::{
        SystemDescription,
        madt::{
            CpuInfo, GicCpuInterface, GicDistributor, GicIts, GicRedistributor, MADT_GICC,
            MADT_GICD, MADT_GICR, MADT_ITS,
        },
        rsdp::{XsdtIter, find_rsdp_in_slice},
    },
    bytes_to_human_readable,
    cpu_interface::{Mpidr, SecondaryBootArgs, mpidr_affinities, mpidr_key},
    exception::ExceptionHandler,
    smccc::cpu_on,
    vcpu::{add_cpu, with_cpus},
    vec::{DynVec, PMVec, RawVec, StaticVec},
    vm::{
        DMAP_START, MAIR_NORMAL_INDEX, MemoryRegion, MemoryRegionType, PAGE_SIZE, TABLE_ENTRIES,
        TTable, align_down, align_up, dmap_addr_to_phys,
        map::map_region,
        page::{PageAllocator, table_allocator::PMTableAllocator},
        phys_addr_to_dmap,
        slab::SlabAllocator,
    },
};
use protocol::BootInfo;
use uefi::{
    boot::{MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::{MemoryMap, MemoryMapMut, MemoryMapOwned},
};

use crate::{
    allocator::KernelPTAllocator,
    earlyinit::{
        earlycon::{EARLYCON, EarlyCon},
        mmu::init_mmu,
        smp::{secondary_entry, secondary_init},
    },
};

struct Exceptions;
impl ExceptionHandler for Exceptions {}

klib::exception_handlers!(Exceptions);

#[global_allocator]
pub static KALLOCATOR: SlabAllocator = SlabAllocator::new();

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

    let boot_info: BootInfo = unsafe { boot_info_ref.assume_init() };

    let kbase = unsafe { &__KBASE as *const _ as usize };
    let offset = kbase - boot_info.kernel_load_physical_address;

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
                base: DMAP_START + entry.phys_start as usize,
                size: (entry.page_count as usize * UEFI_PS),
                region_type: MemoryRegionType::AcpiTables,
            });
        }
    }
    earlycon_writeln!("total: {:#x}", total);

    init_mmu(boot_info.kernel_load_physical_address, offset);

    let page_allocator = &arm_init(uefi_mmap, boot_info.kernel_regions, boot_info.root_pt);

    // SAFETY: this function never returns, so `page_allocator` can be treated as a static reference.
    let page_alloc_ref: &'static PageAllocator = unsafe { mem::transmute(page_allocator) };

    unsafe { KALLOCATOR.init(page_alloc_ref) };

    // ditch lower half mappings
    TTBR0_EL1.set(0);

    let acpi_reg = acpi_reg_opt.expect("no ACPI tables found.");
    let slice = unsafe { from_raw_parts(acpi_reg.base as *const u8, acpi_reg.size) };

    // scanning for the RSDP could be avoided by passing a pointer from UEFI via the boot protocol,
    // but that creates dependence on ACPI (which i'd really rather avoid).
    let rsdp = find_rsdp_in_slice(slice).expect("RSDP not found in ACPI tables.");
    earlycon_writeln!("ACPI RSDP @ {:p}", rsdp);

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

    if let Some(madt) = sys.madt {
        earlycon_writeln!("MADT found");

        for (type_, data) in madt.entries() {
            match type_ {
                MADT_GICC => {
                    if data.len() >= mem::size_of::<GicCpuInterface>() {
                        let gicc = unsafe { &*(data.as_ptr() as *const GicCpuInterface) };
                        let flags = gicc.flags();

                        let enabled = (flags & 0x1) != 0;
                        let online_capable = (flags & 0x8) != 0;

                        add_cpu(CpuInfo {
                            acpi_cpu_uid: gicc.acpi_cpu_uid(),
                            mpidr: gicc.mpidr(),
                            available: enabled || online_capable,
                            //efficiency_class: gicc.efficiency_class(),
                        });

                        let mpidr = unsafe { ptr::read_unaligned(&raw const gicc.mpidr) };
                        earlycon_writeln!("     GICC CPU MPIDR={:#x}", mpidr);
                    }
                }
                MADT_GICD => {
                    if data.len() >= mem::size_of::<GicDistributor>() {
                        let gicd = unsafe { &*(data.as_ptr() as *const GicDistributor) };
                        let base = unsafe { ptr::read_unaligned(&raw const gicd.phys_base) };
                        earlycon_writeln!("     GICD Distributor Base={:#x}", base);
                    }
                }
                MADT_GICR => {
                    if data.len() >= mem::size_of::<GicRedistributor>() {
                        let gicr = unsafe { &*(data.as_ptr() as *const GicRedistributor) };
                        let base = gicr.discovery_range_base();
                        earlycon_writeln!("     GICR Redistributor Base={:#x}", base);
                    }
                }
                MADT_ITS => {
                    if data.len() >= mem::size_of::<GicIts>() {
                        let its = unsafe { &*(data.as_ptr() as *const GicIts) };
                        let id = unsafe { ptr::read_unaligned(&raw const its.translation_id) };
                        let base = unsafe { ptr::read_unaligned(&raw const its.phys_base) };
                        earlycon_writeln!("     ITS ID={}, Base={:#x}", id, base);
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

    if let Some(gtdt) = sys.gtdt {
        let virt = gtdt.virt_el1_gsiv();
        let phys = gtdt.ns_el1_gsiv();
        earlycon_writeln!("GTDT timer GSIVs Virt={}, Phys={}", virt, phys);
    }

    let this_mpidr = Mpidr::current();
    let is_uniprocessor = MPIDR_EL1::U.read(this_mpidr.affinity_only()) == 1;
    earlycon_writeln!("Uniprocessor?: {}", is_uniprocessor);

    if is_uniprocessor {
        unimplemented!();
    }

    let table_allocator = KernelPTAllocator {};

    with_cpus(|count, cpus| {
        earlycon_writeln!("count: {}, cpus: {:#?}", count, cpus);
        for cpu in cpus {
            if cpu.mpidr == this_mpidr.affinity_only() {
                continue;
            }
            earlycon_writeln!("secondary cpu: {:#?}", cpu);

            let mpidr_tuple = mpidr_affinities(cpu.mpidr);
            let mpidr = Mpidr::new(mpidr_tuple.0, mpidr_tuple.1, mpidr_tuple.2, mpidr_tuple.3);

            let bad_stack = Box::new([0u8; 4096]);

            let mut page_tables = Box::new(TTable::<TABLE_ENTRIES>::new());
            let paddr = align_down(boot_info.kernel_load_physical_address, PAGE_SIZE);
            earlycon_writeln!(
                "root: {:p}, paddr: {:#x}, size: {:#x}",
                page_tables.as_mut(),
                paddr,
                boot_info.kernel_size
            );
            map_region(
                page_tables.as_mut(),
                paddr,
                paddr,
                boot_info.kernel_size,
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
                ttbr0: dmap_addr_to_phys(Box::leak(page_tables) as *const _ as u64),
                ttbr1: TTBR1_EL1.get(),
                tcr: secondary_tcr.value,
                mair: MAIR_EL1.get(),
                stack_top_virt: Box::leak(bad_stack) as *const _ as u64,
                entry_virt: secondary_init as *const () as u64,
                sctlr: SCTLR_EL1.get(),
            });

            earlycon_writeln!("args: {:#x?}", args);

            let result = cpu_on(
                true,
                mpidr,
                kaddr_to_paddr(
                    boot_info.kernel_load_physical_address,
                    secondary_entry as *const () as usize,
                ) as u64,
                dmap_addr_to_phys(Box::leak(args) as *const _ as u64),
            );

            earlycon_writeln!(
                "cpu_on (entry: {:#x}) result: {:?}",
                kaddr_to_paddr(
                    boot_info.kernel_load_physical_address,
                    secondary_entry as *const () as usize,
                ) as u64,
                result
            );
        }
    });

    let mut bufs = [[0u8; 16]; 2];
    let bufs_tuple = bufs.split_at_mut(1);

    earlycon_writeln!(
        "heap usage: {} / {}",
        bytes_to_human_readable(KALLOCATOR.heap_usage() as u64, &mut bufs_tuple.0[0]),
        bytes_to_human_readable(KALLOCATOR.heap_capacity() as u64, &mut bufs_tuple.1[0]),
    );

    busy_loop()
}

pub extern "C" fn arm_init(
    uefi_mmap: &mut MemoryMapOwned,
    memory_regions: StaticVec<MemoryRegion>,
    mut root_pt: NonNull<TTable<TABLE_ENTRIES>>,
) -> PageAllocator {
    unsafe {
        asm!(
            "adr {x}, vector_table_el1",
            "msr vbar_el1, {x}",
            x = out(reg) _,
            options(nomem, nostack),
        );
    }

    let mut pmvec: PMVec<MemoryRegion> = PMVec::new();

    {
        let slice = memory_regions.as_slice();
        pmvec.extend_from_slice(slice);
    }

    for &region in uefi_mmap.entries() {
        _ = pmvec.remove_containing(region.phys_start as usize);
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
        pmvec.push(MemoryRegion {
            base: region.phys_start as usize,
            size: (region.page_count as usize * UEFI_PS),
            region_type,
        });
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

    {
        let mut total = 0usize;
        for &region in pmvec_copy.as_slice() {
            //if !region.is_normal() {
            //    continue;
            //};
            if region.size < PAGE_SIZE {
                continue;
            }
            //earlycon_writeln!(
            //    "MemoryRegion {{ base: {:#x}, size: {:#x}, region_type: {:?} }}",
            //    region.base,
            //    region.size,
            //    region.region_type
            //);
            let aligned_base = align_up(region.base, PAGE_SIZE);
            unsafe {
                map_region(
                    root_pt.as_mut(),
                    aligned_base,
                    DMAP_START + aligned_base,
                    align_down(region.size, PAGE_SIZE),
                    AccessPermission::PrivilegedReadWrite,
                    Shareability::InnerShareable,
                    true,
                    true,
                    MAIR_NORMAL_INDEX,
                    &early_page_allocator,
                )
            };
            total += region.size;
        }
        earlycon_writeln!("mem size: {:#x} ({} B)", total, total);
    }

    let mmap_swiss_cheese = early_page_allocator.free_regions.into_inner();

    for &region in mmap_swiss_cheese.as_slice() {
        if let Some(popped) = pmvec_copy.remove_containing(region.base) {
            pmvec_copy.push(region);
        }
    }
    pmvec_copy.compact();

    //earlycon_writeln!("pmvec_copy: {:#x?}", pmvec_copy.as_slice());

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
