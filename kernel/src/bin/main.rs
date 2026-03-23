#![no_std]
#![no_main]
#![feature(fn_traits)]

mod earlyinit;

extern crate alloc;

use aarch64_cpu::{
    asm::{
        barrier::{self, isb},
        wfe,
    },
    registers::{CPACR_EL1, DAIF, MPIDR_EL1, ReadWriteable, Readable, Writeable},
};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::vec::Vec;
use core::{
    arch::{asm, naked_asm},
    fmt::Write,
    mem::{self, MaybeUninit},
    ops::Add,
    panic::PanicInfo,
    ptr::{self, NonNull},
};
use klib::{
    bytes_to_human_readable,
    exception::ExceptionHandler,
    vec::{DynVec, PMVec, RawVec, StaticVec},
    vm::{
        DMAP_START, MAIR_NORMAL_INDEX, MemoryRegion, MemoryRegionType, PAGE_SIZE, TABLE_ENTRIES,
        TTable, align_down, align_up,
        map::map_region,
        page::{
            PageAllocator,
            table_allocator::{KernelPTAllocator, PMTableAllocator},
        },
        slab::SlabAllocator,
    },
};
use protocol::BootInfo;
use uefi::{
    boot::{MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::{MemoryMap, MemoryMapMut, MemoryMapOwned},
};

use crate::earlyinit::{
    earlycon::{EARLYCON, EarlyCon},
    mmu::init_mmu,
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

fn kentry(boot_info_ref: MaybeUninit<BootInfo>) -> ! {
    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);

    let mpidr = MPIDR_EL1.get();
    let core_id = (mpidr & 0xFF) as u8;

    if core_id != 0 {
        busy_loop();
    }

    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);

    let boot_info: BootInfo = unsafe { boot_info_ref.assume_init() };

    let kbase = unsafe { &__KBASE as *const _ as usize };
    let offset = kbase - boot_info.kernel_load_physical_address;

    {
        let mut lock = EARLYCON.lock();
        *lock = Some(EarlyCon::new(boot_info.serial_uart_address));
    }

    let uefi_mmap = &mut unsafe { ptr::read(&boot_info.memory_map) };
    uefi_mmap.sort();
    earlycon_writeln!("uefi_mmap @ {:#x}", uefi_mmap as *const _ as u64);

    let mut total = 0usize;
    for entry in uefi_mmap.entries() {
        match entry.ty {
            _ => {
                earlycon_writeln!(
                    "MemoryDescriptor {{ phys_start: {:#x}, size: {:#x}, ty: {:?} }}",
                    entry.phys_start,
                    entry.page_count as usize * UEFI_PS,
                    entry.ty
                );
                total += entry.page_count as usize * UEFI_PS;
            }
            _ => {}
        }
    }
    earlycon_writeln!("total: {:#x}", total);

    init_mmu(boot_info.kernel_load_physical_address, offset);
    earlycon_writeln!("hi");

    let page_allocator = &arm_init(uefi_mmap, boot_info.kernel_regions, boot_info.root_pt);

    let page_alloc_ref: &'static PageAllocator = unsafe { mem::transmute(page_allocator) };

    unsafe { KALLOCATOR.init(page_alloc_ref) };

    let mut vec = Vec::new();
    vec.push(42);

    earlycon_writeln!("heap vec: {:?}", vec);

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
            earlycon_writeln!(
                "MemoryRegion {{ base: {:#x}, size: {:#x}, region_type: {:?} }}",
                region.base,
                region.size,
                region.region_type
            );
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

    earlycon_writeln!("pmvec_copy: {:#x?}", pmvec_copy.as_slice());

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
        "page start: {:#x}, head: {:#x}",
        page as usize,
        (page as usize).add(PAGE_SIZE)
    );

    page_allocator.free_pages(page);

    page_allocator
}
