#![no_std]
#![no_main]
#![feature(negative_impls)]

extern crate alloc;

mod allocator;
mod earlyinit;
mod interrupt;
mod log;
mod lut;

use ::log::{debug, trace};
use aarch64_cpu::asm::wfe;
use atomic_refcell::AtomicRefCell;
use core::{
    arch::{asm, naked_asm},
    mem::MaybeUninit,
    panic::PanicInfo,
};
use klib::{
    bytes_to_human_readable,
    hardware::device::DeviceTree,
    pm::page::PageAllocator,
    register_drivers,
    scheduler::Scheduler,
    vm::{slab::SlabAllocator, user::address_space::AddressSpace},
};
use protocol::BootInfo;

use crate::{
    allocator::KernelAddressTranslator,
    earlyinit::{
        mmu::init_cpu,
        platform::{BootInfoInitToken, uefi_arm64_bootstrap},
    },
};

use self::{
    allocator::KernelPTAllocator,
    earlyinit::{earlycon::EARLYCON, exception::Exceptions},
};

klib::exception_handlers!(Exceptions);

static DEVICE_TREE: AtomicRefCell<DeviceTree> = AtomicRefCell::new(DeviceTree::new());

// use `KALLOCATOR`
static KPAGE_ALLOCATOR: PageAllocator = PageAllocator::new(&KernelAddressTranslator);

#[global_allocator]
static KALLOCATOR: SlabAllocator = SlabAllocator::new(&KPAGE_ALLOCATOR, &KernelAddressTranslator);

static KPT_ALLOCATOR: KernelPTAllocator = KernelPTAllocator {};

static GLOBAL_SCHEDULER: Scheduler = Scheduler::new();

static KERNEL_ADDRESS_SPACE: AddressSpace = unsafe {
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
        EARLYCON.steal();
        earlycon_writeln!("{}", info);
    }
    busy_loop();
}

register_drivers!([]);

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
    static __KBASE: usize;
}

const STACK_SIZE: usize = 32 * 1024;

#[allow(dead_code)]
#[repr(align(16))]
struct KStack([u8; STACK_SIZE]);

impl KStack {
    const fn new() -> Self {
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

    let boot_info_init_token = BootInfoInitToken::new().unwrap();
    let mut boot_info_token = unsafe { boot_info_init_token.init(boot_info_ref) }.unwrap();

    let r = boot_info_token.get();
    let m = boot_info_token.get_mut();

    uefi_arm64_bootstrap(boot_info_token);

    {
        let mut lock = EARLYCON.lock();
        if let Some(uart) = &mut *lock {
            _ = uart;
            // TODO: correctly map the rest of MMIO into DMAP
            //uart.switch(KernelAddressTranslator.phys_to_dmap(boot_info.serial_uart_address) as _);
        }
    }

    debug!("dead end");

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
