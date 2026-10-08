#![no_std]
#![no_main]
#![feature(negative_impls)]
#![feature(sync_unsafe_cell)]

extern crate alloc;

mod earlyinit;
mod log;
mod lut;

use atomic_refcell::AtomicRefCell;
use core::{alloc::GlobalAlloc, panic::PanicInfo};
use hal::interrupt::InterruptGuard;
use klib::{
    cpu_interface::CpuTopologyId, hardware::device::DeviceTree, register_drivers, vm::KALLOCATOR,
};
use protocol::BootInfo;

use crate::earlyinit::{
    idle::idle_init,
    mmu::init_cpu,
    platform::{BootInfoInitToken, uefi_arm64_bootstrap},
};

hal::kernel_entry!(kentry);

static DEVICE_TREE: AtomicRefCell<DeviceTree> = AtomicRefCell::new(DeviceTree::new());

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    InterruptGuard::disable();
    let core = CpuTopologyId::current();
    earlyinit::earlycon::earlycon_panic_write(format_args!(
        "CPU MPIDR={} PANIC: {}\r\n",
        core, info
    ));
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
        hal::boot::wait();
    }
}

#[allow(dead_code)]
fn busy_loop_ret() {
    loop {
        hal::boot::wait();
    }
}

unsafe extern "C" {
    static __KBASE: usize;
}

unsafe extern "C" fn kentry(boot_info_address: usize) -> ! {
    let boot_info_ref = boot_info_address as *mut BootInfo;
    init_cpu();

    earlyinit::shell::initialize_uptime();

    unsafe { hal::exception::install(earlyinit::exception::handle) };

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
