use core::{
    sync::atomic::{AtomicU8, Ordering},
    usize,
};

use crate::{cpu_interface::CpuIdLogical, pm::page::mapper::AddressTranslator, this_cpu};

use super::{
    context::RegisterFileRef,
    cpu_interface::CpuTopologyId,
    pm::page::mapper::TableAllocator,
    sync::{Mutex, RwLock},
    thread::{Thread, ThreadState},
    vm::page_allocator::PhysicalPageAllocator,
};

use aarch64_cpu::{
    asm::barrier::{self, isb},
    registers::{TPIDR_EL1, TTBR0_EL1, Writeable},
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    vec::Vec,
};

#[derive(Debug)]
pub struct LocalScheduler<'a> {
    thread_queue: VecDeque<Arc<Thread<'a>>>,
    current_thread: Option<Arc<Thread<'a>>>,
}

impl LocalScheduler<'_> {
    pub const fn new() -> Self {
        Self {
            thread_queue: VecDeque::new(),
            current_thread: None,
        }
    }
}

#[derive(Debug)]
pub struct Scheduler<'a> {
    queues: RwLock<Vec<Mutex<LocalScheduler<'a>>>>,
    spawn_counter: AtomicU8,
}

unsafe impl Send for Scheduler<'_> {}
unsafe impl Sync for Scheduler<'_> {}

impl<'a> Scheduler<'a> {
    pub const fn new() -> Self {
        Self {
            queues: RwLock::new(Vec::new()),
            spawn_counter: AtomicU8::new(0),
        }
    }

    pub fn register_cpu(&self, cpu_id: CpuIdLogical) {
        let mut queues = self.queues.write();
        if cpu_id.to_usize() >= queues.len() {
            queues.resize_with(cpu_id.to_usize() + 1, || Mutex::new(LocalScheduler::new()));
        }
    }

    pub fn spawn(&self, thread: Arc<Thread<'a>>) {
        let queues = self.queues.read();
        assert!(!queues.is_empty(), "scheduler has no CPUs");

        let counter = self.spawn_counter.fetch_add(1, Ordering::AcqRel);
        let cpu_i = counter as usize % queues.len();
        let target_queue = &queues[cpu_i];

        thread.set_state(ThreadState::Ready);
        target_queue.lock().thread_queue.push_back(thread);
    }

    pub fn schedule<'ctx>(&self, ctx: RegisterFileRef<'ctx>) -> RegisterFileRef<'ctx> {
        let cpu_id = CpuIdLogical::current();
        let queues_guard = self.queues.read();
        let queue_mutex = &queues_guard[cpu_id.to_usize()];
        let mut local_queue = queue_mutex.lock();

        let prev_thread = local_queue.current_thread.take();

        if let Some(ref prev) = prev_thread {
            prev.with_ctx_mut(|prev_ctx| {
                *prev_ctx = *ctx;
            });

            if prev.get_state() == ThreadState::Running {
                prev.set_state(ThreadState::Ready);
                local_queue.thread_queue.push_back(prev.clone());
            }
        }

        let next_thread = local_queue
            .thread_queue
            .pop_front()
            .or_else(|| prev_thread.clone());

        if let Some(next) = next_thread {
            if let Some(prev) = &prev_thread {
                if Arc::ptr_eq(prev, &next) {
                    local_queue.current_thread = Some(next);
                    return ctx;
                }
            }

            next.set_state(ThreadState::Running);
            this_cpu!();

            // TPIDR_EL1.set(Arc::as_ptr(&next) as u64);

            if let Some(process) = next.process() {
                process.with_address_space(|addr_space| {
                    let root = addr_space.root.as_ptr();
                    let addr = root as *const _ as u64;
                    TTBR0_EL1.set_baddr(addr);
                    isb(barrier::SY);
                });
            }

            local_queue.current_thread = Some(next.clone());

            let next_ptr = next.with_ctx_mut(|next_ctx| next_ctx as *mut _);

            // theoretically this is safe. nothing else should mutate the `ctx`
            unsafe { RegisterFileRef(&mut *next_ptr) }
        } else {
            ctx
        }
    }
}
