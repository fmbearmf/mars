use core::sync::atomic::Ordering;

use alloc::sync::Arc;
use hal::interrupt::InterruptGuard;
use klib::{scheduler::GLOBAL_SCHEDULER, stack::Stack, this_cpu, thread::Thread};

/// call per-core
pub fn idle_init() -> ! {
    let idle_stack = Stack::default();

    let idle_thread = Arc::new(Thread::new_kernel(idle_stack, idle_entry, u8::MIN));

    // bind this stack to its processor before enabling preemption
    let mut context = unsafe { GLOBAL_SCHEDULER.start_kernel(idle_thread) };
    unsafe { hal::exception::resume(&mut context) }
}

extern "C" fn idle_entry(_: usize) -> ! {
    use log::*;

    this_cpu!().ready.store(true, Ordering::Release);

    debug!("Core {} online & going idle.", this_cpu!().id);
    // device initialization installs interrupt delivery before entering idle
    unsafe { InterruptGuard::enable() };

    loop {
        if GLOBAL_SCHEDULER.prepare_idle() {
            klib::scheduler::Scheduler::yield_now();
        } else {
            hal::boot::wait();
        }
    }
}
