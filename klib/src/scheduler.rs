use super::{
    context::RegisterFile,
    cpu_interface::Mpidr,
    sync::RwLock,
    thread::{Thread, ThreadId, ThreadState},
};

extern crate alloc;

use aarch64_cpu::{
    asm::barrier::{self, isb},
    registers::TTBR0_EL1,
};
use alloc::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

pub static SCHEDULER: Scheduler<'static> = Scheduler::new();

#[derive(Debug)]
pub struct Scheduler<'a> {
    thread_queue: RwLock<VecDeque<Arc<Thread<'a>>>>,
    current_threads: RwLock<BTreeMap<u64, Arc<Thread<'a>>>>,
}

impl<'a> Scheduler<'a> {
    pub const fn new() -> Self {
        Self {
            thread_queue: RwLock::new(VecDeque::new()),
            current_threads: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn spawn(&self, thread: Arc<Thread<'a>>) {
        let mut tq = self.thread_queue.write();
        thread.set_state(ThreadState::Ready);
        tq.push_back(thread);
    }

    pub fn schedule(&self, ctx: &mut RegisterFile) {
        let cpu_id = Mpidr::current().affinity_only();

        let mut tq = self.thread_queue.write();
        let mut current = self.current_threads.write();

        let prev_thread = current.remove(&cpu_id);

        if let Some(ref prev) = prev_thread {
            prev.with_ctx_mut(|prev_ctx| {
                *prev_ctx = *ctx;
            });

            if prev.get_state() == ThreadState::Running {
                prev.set_state(ThreadState::Ready);
                tq.push_back(prev.clone());
            }
        }

        let next_thread = tq.pop_front().or(prev_thread);

        if let Some(next) = next_thread {
            next.set_state(ThreadState::Running);

            next.with_ctx_mut(|next_ctx| {
                *ctx = *next_ctx;
            });

            if let Some(process) = next.process() {
                let ttbr0 = process.address_space() as *const _ as u64;
                TTBR0_EL1.set_baddr(ttbr0);

                isb(barrier::SY);
            }

            current.insert(cpu_id, next);
        } else {
            // uhhh idle thread here
        }
    }
}
