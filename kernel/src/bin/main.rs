#![no_std]
#![no_main]

mod earlyinit;

use aarch64_cpu::{
    asm::{
        barrier::{self, isb},
        wfe,
    },
    registers::{CPACR_EL1, DAIF, MPIDR_EL1, ReadWriteable, Readable, Writeable},
};
use core::{
    arch::{asm, naked_asm},
    cmp,
    fmt::Write,
    mem::{self, MaybeUninit},
    ops::Add,
    panic::PanicInfo,
    ptr, slice,
};
use klib::{
    exception::ExceptionHandler,
    vec::{RawVec, StaticVec},
    vm::{
        DMAP_START, MemoryRegion, PAGE_SIZE, align_down, align_up,
        map::TableAllocator,
        page::{PageAllocator, table_allocator::KernelPTAllocator},
    },
};
use protocol::BootInfo;
use uefi::{
    boot::{MemoryDescriptor, MemoryType, PAGE_SIZE as UEFI_PS},
    mem::memory_map::{MemoryMap, MemoryMapMut, MemoryMapOwned},
};

use crate::earlyinit::{
    earlycon::{EARLYCON, EarlyCon},
    mmu::init_mmu,
};

struct Exceptions;
impl ExceptionHandler for Exceptions {}

klib::exception_handlers!(Exceptions);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        EARLYCON.force_unlock();
        earlycon_writeln!("{}", info);
    }
    busy_loop();
}

fn busy_loop() -> ! {
    loop {
        wfe();
    }
}

fn busy_loop_ret() {
    loop {
        wfe();
    }
}

unsafe extern "C" {
    pub static __KBASE: usize;
}

const STACK_SIZE: usize = 128 * 1024;
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
            MemoryType::LOADER_CODE
            | MemoryType::BOOT_SERVICES_DATA
            | MemoryType::BOOT_SERVICES_CODE => {
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

    arm_init(uefi_mmap, boot_info.kernel_regions);

    busy_loop()
}

pub extern "C" fn arm_init(
    uefi_mmap: &mut MemoryMapOwned,
    memory_regions: StaticVec<MemoryRegion>,
) {
    unsafe {
        asm!(
            "adr {x}, vector_table_el1",
            "msr vbar_el1, {x}",
            x = out(reg) _,
            options(nomem, nostack),
        );
    }

    let mut page_allocator = unsafe { PageAllocator::init(&[]) };

    for &region in memory_regions.as_slice() {
        if !region.is_normal() {
            continue;
        };
        if region.size <= 2 * PAGE_SIZE {
            continue;
        }
        earlycon_writeln!(
            "MemoryRegion {{ base: {:#x}, size: {:#x}, region_type: {:?} }}",
            region.base,
            region.size,
            region.region_type
        );
        page_allocator.add_range(region);
    }

    let pt_allocator = KernelPTAllocator::new(&page_allocator, memory_regions);

    let page = page_allocator.alloc_page() as usize;
    earlycon_writeln!("page start: {:#x}, head: {:#x}", page, page.add(PAGE_SIZE));

    panic!("End.");
}
