#![no_std]
#![no_main]
#![feature(negative_impls)]
#![feature(sync_unsafe_cell)]

extern crate alloc;

mod earlyinit;
mod log;
mod lut;

use aarch64_cpu::asm::{
    barrier::{self, dsb},
    wfe,
};
use atomic_refcell::AtomicRefCell;
use core::{
    alloc::GlobalAlloc,
    arch::{asm, naked_asm},
    panic::PanicInfo,
};
use klib::{
    cpu_interface::CpuTopologyId, guard::InterruptGuard, hardware::device::DeviceTree,
    register_drivers, unsafe_println_panic_only_unsafe, vm::KALLOCATOR,
};
use protocol::BootInfo;

use crate::earlyinit::{
    idle::idle_init,
    mmu::init_cpu,
    platform::{BootInfoInitToken, uefi_arm64_bootstrap},
};

use self::earlyinit::exception::Exceptions;

klib::exception_handlers!(Exceptions);

static DEVICE_TREE: AtomicRefCell<DeviceTree> = AtomicRefCell::new(DeviceTree::new());

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    InterruptGuard::disable();
    unsafe {
        let core = CpuTopologyId::current();
        unsafe_println_panic_only_unsafe!("CPU MPIDR={} PANIC: {}", core, info);
    }
    busy_loop()
}

struct GlobalAllocWrapper;

unsafe impl GlobalAlloc for GlobalAllocWrapper {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        unsafe { KALLOCATOR.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        unsafe {
            KALLOCATOR.dealloc(ptr, layout);
        }
    }

    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: core::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        unsafe { KALLOCATOR.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
pub(self) static GLOBAL_ALLOCATOR: GlobalAllocWrapper = GlobalAllocWrapper;

register_drivers!([]);

/// "Overwatch stopped our train in the woods and took my husband for questioning.
/// They said he'd be back on the next train. I'm not sure when that was.
/// They're being nice, though, letting me wait for him." (cit_fence_woods)
#[allow(dead_code)]
fn busy_loop() -> ! {
    loop {
        wfe();
        dsb(barrier::SY); // YIELD (i.e. core::hint::spin_loop())
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

const STACK_SIZE: usize = 16 * 1024;

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
    let boot_info_token = unsafe { boot_info_init_token.init(boot_info_ref) }.unwrap();

    uefi_arm64_bootstrap(boot_info_token);

    idle_init()
}

// fn print_mem_usage() {
//     let mut bufs = [[0u8; 16]; 2];
//     let bufs_tuple = bufs.split_at_mut(1);
//
//     trace!(
//         "page usage: {} / {}",
//         bytes_to_human_readable(KALLOCATOR.page_usage() as u64, &mut bufs_tuple.0[0]),
//         bytes_to_human_readable(KALLOCATOR.capacity() as u64, &mut bufs_tuple.1[0]),
//     );
// }
