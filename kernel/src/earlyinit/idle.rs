use core::sync::atomic::Ordering;

use alloc::sync::Arc;
use klib::{
    context::RegisterFileRef, guard::InterruptGuard, stack::Stack, this_cpu, thread::Thread,
};

use crate::{GLOBAL_SCHEDULER, busy_loop};

unsafe extern "C" {
    fn el1_load_register_file(regs: RegisterFileRef<'_>) -> !;
}

/// call per-core
pub fn idle_init() -> ! {
    let idle_stack = Stack::default();

    let idle_thread = Arc::new(Thread::new_kernel(
        idle_stack,
        idle_entry as *const (),
        u8::MIN,
    ));

    let regs = idle_thread.with_ctx_mut(|next| next as *mut _);

    GLOBAL_SCHEDULER.spawn(idle_thread);

    unsafe {
        let regs = RegisterFileRef(&mut *regs);
        el1_load_register_file(regs)
    }
}

fn idle_entry() -> ! {
    use log::trace;
    trace!("core {} idle entry.", this_cpu!().id);

    this_cpu!().ready.store(true, Ordering::Release);

    trace!("core {} post-ready", this_cpu!().id);
    InterruptGuard::enable();

    busy_loop()
}
