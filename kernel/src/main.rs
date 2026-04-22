#![no_std]
#![no_main]

extern crate alloc;

mod allocator;
mod earlyinit;
mod log;

use ::log::{LevelFilter, debug, info, trace};
use aarch64_cpu::asm::wfe;
use core::{
    arch::{asm, naked_asm},
    mem::MaybeUninit,
    panic::PanicInfo,
    ptr::{self},
};
use klib::{
    bytes_to_human_readable,
    pm::page::PageAllocator,
    register_drivers,
    scheduler::Scheduler,
    vm::{
        slab::SlabAllocator,
        user::{PAGE_DESCRIPTORS, address_space::AddressSpace},
    },
};
use protocol::BootInfo;
use uefi::mem::memory_map::{MemoryMap, MemoryMapMut};

use crate::{
    allocator::KernelAddressTranslator,
    earlyinit::{
        acpi::acpi_init,
        mem::{
            clone_and_process_mmap, create_page_descriptors, populate_alloc_stage0,
            switch_to_new_page_tables,
        },
        mmu::init_cpu,
        platform::uefi_arm64_bootstrap,
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

register_drivers!([mars_acpi_driver::DRIVER]);

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

    uefi_arm64_bootstrap(boot_info_ref);

    {
        let mut lock = EARLYCON.lock();
        if let Some(uart) = &mut *lock {
            // TODO: correctly map the rest of MMIO into DMAP
            //uart.switch(KernelAddressTranslator.phys_to_dmap(boot_info.serial_uart_address) as _);
        }
    }

    debug!("weldington");

    busy_loop();
}

fn print_mem_usage() {
    let mut bufs = [[0u8; 16]; 2];
    let bufs_tuple = bufs.split_at_mut(1);

    trace!(
        "page usage: {} / {}",
        bytes_to_human_readable(KALLOCATOR.page_usage() as u64, &mut bufs_tuple.0[0]),
        bytes_to_human_readable(KALLOCATOR.capacity() as u64, &mut bufs_tuple.1[0]),
    );
}

pub extern "C" fn alloc_init() -> PageAllocator<'static> {
    let page_allocator = PageAllocator::new(&KernelAddressTranslator);

    page_allocator
}
