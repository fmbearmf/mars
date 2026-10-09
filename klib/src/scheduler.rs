#[cfg(test)]
mod tests;

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::cpu_interface::CpuIdLogical;

mod ready_pool;

use ready_pool::{READY_POOL_SIZE, ReadyPool};

use super::{
    context::RegisterFile,
    sync::{RwLock, UnfairSpinlock},
    thread::{Thread, ThreadMonitorSnapshot, ThreadState},
};

use crate::{process::Process, vm::user::address_space::KERNEL_ADDRESS_SPACE};
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};

pub struct LocalScheduler<'a> {
    idle: Option<Arc<Thread<'a>>>,
    current_thread: Option<Arc<Thread<'a>>>,
    deferred: Option<Arc<Thread<'a>>>,
    current_process: Option<Arc<Process<'a>>>,
    deferred_process: Option<Arc<Process<'a>>>,
}

impl LocalScheduler<'_> {
    pub const fn new() -> Self {
        Self {
            idle: None,
            current_thread: None,
            deferred: None,
            current_process: None,
            deferred_process: None,
        }
    }
}

struct CpuScheduler<'a> {
    ready: ReadyPool<Thread<'a>>,
    enqueue: UnfairSpinlock<()>,
    local: UnfairSpinlock<LocalScheduler<'a>>,
}

impl CpuScheduler<'_> {
    const fn new() -> Self {
        Self {
            ready: ReadyPool::new(),
            enqueue: UnfairSpinlock::new(()),
            local: UnfairSpinlock::new(LocalScheduler::new()),
        }
    }
}

pub static GLOBAL_SCHEDULER: Scheduler = Scheduler::new();

pub struct Scheduler<'a> {
    queues: RwLock<Vec<CpuScheduler<'a>>>,
    injector: UnfairSpinlock<VecDeque<Arc<Thread<'a>>>>,
    spawn_counter: AtomicUsize,
    dequeue_counter: AtomicUsize,
    steal_events: UnfairSpinlock<StealTrace>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CpuSchedulerSnapshot {
    pub thread: ThreadMonitorSnapshot,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct StealEvent {
    pub sequence: usize,
    pub source_cpu: u16,
    pub destination_cpu: u16,
    pub thread_id: u32,
}

struct StealTrace {
    next: usize,
    events: [u64; 256],
}

// safety: all scheduler state is protected by locks or atomics, and queued threads are shared through `Arc`
unsafe impl Send for Scheduler<'_> {}
// safety: scheduler mutations use synchronized queues, locks, and atomics across processors
unsafe impl Sync for Scheduler<'_> {}

impl<'a> Scheduler<'a> {
    pub const fn new() -> Self {
        Self {
            queues: RwLock::new(Vec::new()),
            injector: UnfairSpinlock::new(VecDeque::new()),
            spawn_counter: AtomicUsize::new(0),
            dequeue_counter: AtomicUsize::new(0),
            steal_events: UnfairSpinlock::new(StealTrace {
                next: 0,
                events: [0; 256],
            }),
        }
    }

    fn publish(&self, cpu: usize, thread: Arc<Thread<'a>>) {
        let _interrupts = hal::interrupt::InterruptGuard::new();
        let queues = self.queues.read();

        {
            let _enqueue = queues[cpu].enqueue.lock();
            if let Err(thread) = queues[cpu].ready.push(thread) {
                self.injector.lock().push_back(thread);
            }
        }

        drop(queues);
        hal::boot::notify();
    }

    fn take_ready(&self, cpu: usize, queues: &[CpuScheduler<'a>]) -> Option<Arc<Thread<'a>>> {
        if self.dequeue_counter.fetch_add(1, Ordering::Relaxed) & 1 != 0 {
            if let Some(thread) = self.injector.lock().pop_front() {
                return Some(thread);
            }
        }

        if let Some(thread) = queues[cpu].ready.pop() {
            return Some(thread);
        }

        for offset in 1..queues.len() {
            let victim = (cpu + offset) % queues.len();
            if let Some(thread) = queues[victim].ready.pop() {
                self.record_steal(victim, cpu, thread.thread_id());
                return Some(thread);
            }
        }

        self.injector.lock().pop_front()
    }

    fn record_steal(&self, source: usize, destination: usize, thread_id: u32) {
        if source > u16::MAX as usize || destination > u16::MAX as usize {
            return;
        }

        let mut trace = self.steal_events.lock();

        let sequence = trace.next;
        let index = sequence % trace.events.len();
        let packed = (u64::from(source as u16) << 48)
            | (u64::from(destination as u16) << 32)
            | u64::from(thread_id);

        trace.events[index] = packed;
        trace.next = sequence.wrapping_add(1);
    }

    pub fn steal_cursor(&self) -> usize {
        self.steal_events.lock().next
    }

    /// copy recent successful local-queue steals after `cursor`
    pub fn steal_events_since(&self, cursor: usize, output: &mut [StealEvent]) -> (usize, usize) {
        let trace = self.steal_events.lock();

        let end = trace.next;
        let start = cursor.max(end.saturating_sub(trace.events.len()));

        let mut written = 0;
        for sequence in start..end {
            if written == output.len() {
                break;
            }

            let packed = trace.events[sequence % trace.events.len()];

            output[written] = StealEvent {
                sequence,
                source_cpu: (packed >> 48) as u16,
                destination_cpu: (packed >> 32) as u16,
                thread_id: packed as u32,
            };

            written += 1;
        }

        (start + written, written)
    }

    /// enqueue a batch on one cpu's local queue without falling back to the injector
    pub fn spawn_batch_on_cpu(&self, cpu: usize, threads: &[Arc<Thread<'a>>]) -> Result<(), ()> {
        let _interrupts = hal::interrupt::InterruptGuard::new();

        let queues = self.queues.read();
        let Some(queue) = queues.get(cpu) else {
            return Err(());
        };

        let _enqueue = queue.enqueue.lock();
        if queue.ready.len().saturating_add(threads.len()) > READY_POOL_SIZE {
            return Err(());
        }

        let mut scheduled = 0;
        for thread in threads {
            if !thread.schedule_fresh() {
                for scheduled_thread in &threads[..scheduled] {
                    scheduled_thread.unschedule_fresh();
                }

                return Err(());
            }

            scheduled += 1;
        }

        for thread in threads {
            if let Err(thread) = queue.ready.push(thread.clone()) {
                // capacity is reserved by the enqueue lock and the preceding length check
                thread.unschedule_fresh();
                return Err(());
            }
        }

        drop(_enqueue);
        drop(queues);

        hal::boot::notify();

        Ok(())
    }

    /// wakes a blocked thread and puts it in the ready queue
    pub fn unblock(&self, thread: Arc<Thread<'a>>) {
        let _interrupts = hal::interrupt::InterruptGuard::new();

        if thread.wake() {
            let queues = self.queues.read();

            assert!(!queues.is_empty(), "scheduler has no CPUs");
            let cpu = self.spawn_counter.fetch_add(1, Ordering::Relaxed) % queues.len();
            drop(queues);

            self.publish(cpu, thread);
        }
    }

    /// puts current thread into `wait_queue` as blocked and switches out
    pub fn block_current(&self, waiters: &mut VecDeque<Arc<Thread<'a>>>) {
        let interrupts = hal::interrupt::InterruptGuard::new();

        assert_eq!(
            crate::this_cpu!().exception_depth.load(Ordering::Acquire),
            0,
            "cannot block from an exception"
        );

        let cpu_id = CpuIdLogical::current().to_usize();
        let queues = self.queues.read();
        let local = queues[cpu_id].local.lock();
        let current = local.current_thread.as_ref().expect("no running thread");

        current.block_scheduled();
        waiters.push_back(current.clone());

        drop(local);
        drop(queues);
        drop(interrupts);
    }

    #[inline(always)]
    pub fn yield_now() {
        assert_eq!(
            crate::this_cpu!().exception_depth.load(Ordering::Acquire),
            0,
            "cannot yield from an exception"
        );

        // safety: the caller is outside exception context and this requests a deferred context switch
        unsafe { hal::context::request_reschedule() }
    }

    /// finish deferred ownership on the pinned idle stack and report ready work
    pub fn prepare_idle(&self) -> bool {
        let _interrupts = hal::interrupt::InterruptGuard::new();
        let cpu = CpuIdLogical::current().to_usize();

        assert_eq!(
            crate::this_cpu!().exception_depth.load(Ordering::Acquire),
            0
        );

        let queues = self.queues.read();
        let mut local = queues[cpu].local.lock();

        assert!(
            local
                .idle
                .as_ref()
                .zip(local.current_thread.as_ref())
                .is_some_and(|(idle, current)| Arc::ptr_eq(idle, current))
        );

        if let Some(deferred) = local.deferred.take() {
            if deferred.finish_switch() {
                let _enqueue = queues[cpu].enqueue.lock();
                if let Err(thread) = queues[cpu].ready.push(deferred) {
                    self.injector.lock().push_back(thread);
                }

                hal::boot::notify();
            }
        }

        local.deferred_process = None;
        drop(local);

        let ready =
            queues.iter().any(|queue| queue.ready.has_work()) || !self.injector.lock().is_empty();

        ready
    }

    pub fn current_thread(&self) -> Option<Arc<Thread<'a>>> {
        let _interrupts = hal::interrupt::InterruptGuard::new();
        let cpu_id = CpuIdLogical::current();
        let queues = self.queues.read();

        queues[cpu_id.to_usize()]
            .local
            .lock()
            .current_thread
            .clone()
    }

    /// can be called any number of times
    /// must be called with at least the highest numbered `CpuIdLogical`
    pub fn register_cpu(&self, cpu_id: CpuIdLogical) {
        let _interrupts = hal::interrupt::InterruptGuard::new();
        let mut queues = self.queues.write();

        if cpu_id.to_usize() >= queues.len() {
            queues.resize_with(cpu_id.to_usize() + 1, || CpuScheduler::new());
        }
    }

    /// bind the initial kernel thread to this processor
    ///
    /// safety:
    /// - call with interrupts masked and keep them masked until resuming the returned context
    /// - supply a fresh thread that no other processor or queue can run
    pub unsafe fn start_kernel(&self, thread: Arc<Thread<'a>>) -> RegisterFile {
        assert!(thread.is_kernel(), "initial thread must run in the kernel");

        let context = thread.with_ctx(|context| *context);
        let queues = self.queues.read();
        let mut local = queues[CpuIdLogical::current().to_usize()].local.lock();

        assert!(
            local.current_thread.is_none(),
            "processor already has a running thread"
        );

        thread.start_idle();
        local.idle = Some(thread.clone());
        local.current_thread = Some(thread);

        context
    }

    pub fn try_queue_snapshot(&self) -> Option<(usize, usize)> {
        let _interrupts = hal::interrupt::InterruptGuard::new();

        let queues = self.queues.try_read()?;
        let ready = queues.iter().map(|queue| queue.ready.len()).sum();
        let injector = self.injector.try_lock()?;

        Some((ready, injector.len()))
    }

    /// copy a cpu's live scheduler state without waiting for scheduler locks
    pub fn try_cpu_snapshot(&self, cpu_id: CpuIdLogical) -> Option<CpuSchedulerSnapshot> {
        let _interrupts = hal::interrupt::InterruptGuard::new();
        let queues = self.queues.try_read()?;
        let local = queues.get(cpu_id.to_usize())?.local.try_lock()?;
        let current = local.current_thread.as_ref()?;
        let idle = local
            .idle
            .as_ref()
            .is_some_and(|idle| Arc::ptr_eq(idle, current));
        let mut thread = current.try_monitor_snapshot()?;
        thread.idle = idle;
        Some(CpuSchedulerSnapshot { thread })
    }

    /// terminate the current thread and switch away after dropping the caller's thread reference
    pub fn exit_current() -> ! {
        let interrupts = hal::interrupt::InterruptGuard::new();
        let thread = GLOBAL_SCHEDULER
            .current_thread()
            .expect("no current thread to terminate");

        thread.set_state(ThreadState::Dead);

        drop(thread);
        drop(interrupts);

        Self::yield_now();
        loop {
            Self::yield_now();
        }
    }

    pub fn spawn(&self, thread: Arc<Thread<'a>>) {
        let _interrupts = hal::interrupt::InterruptGuard::new();
        if !thread.schedule_fresh() {
            return;
        }

        let queues = self.queues.read();
        assert!(!queues.is_empty(), "scheduler has no CPUs");

        let cpu = self.spawn_counter.fetch_add(1, Ordering::Relaxed) % queues.len();
        drop(queues);

        self.publish(cpu, thread);
    }

    pub fn schedule(&self, ctx: &mut RegisterFile) {
        let _interrupts = hal::interrupt::InterruptGuard::new();

        assert_eq!(
            crate::this_cpu!().exception_depth.load(Ordering::Acquire),
            1,
            "scheduler requires a non-nested exception"
        );

        let cpu = CpuIdLogical::current().to_usize();
        let queues = self.queues.read();
        let local_mutex = &queues[cpu].local;
        let mut local = local_mutex.lock();

        if let Some(deferred) = local.deferred.take() {
            if deferred.finish_switch() {
                let _enqueue = queues[cpu].enqueue.lock();
                let _ = queues[cpu].ready.push(deferred.clone()).map_err(|thread| {
                    self.injector.lock().push_back(thread);
                });

                hal::boot::notify();
            }
        }

        local.deferred_process = None;

        let previous = local
            .current_thread
            .take()
            .expect("CPU has no current thread");

        previous.save_running_context(*ctx);
        if previous.get_state() == ThreadState::Running {
            previous.set_state(ThreadState::Ready);
        }

        let mut next = None;
        while let Some(candidate) = self.take_ready(cpu, &queues) {
            if candidate.claim_queued() {
                next = Some(candidate);
                break;
            }
        }

        if next.is_none() && previous.get_state() == ThreadState::Ready {
            next = Some(previous.clone());
        }

        if next.is_none() {
            next = local.idle.clone();
        }

        let next = next.expect("CPU has no idle thread");

        if Arc::ptr_eq(&previous, &next) {
            previous.set_state(ThreadState::Running);
            local.current_thread = Some(previous);
            return;
        }

        if local
            .idle
            .as_ref()
            .is_some_and(|idle| Arc::ptr_eq(idle, &next))
        {
            next.resume_idle();
        }

        let next_context = next.context_for_switch();
        let next_process = next.process();

        if let Some(process) = &next_process {
            // safety: the selected process is initialized before its address space is activated
            process.with_address_space(|space| unsafe {
                space
                    .activate()
                    .expect("uninitialized process address space");
            });
        } else {
            // safety: the kernel address space is initialized before scheduled kernel threads run
            unsafe {
                KERNEL_ADDRESS_SPACE
                    .activate()
                    .expect("uninitialized kernel address space")
            };
        }

        local.deferred = Some(previous);
        local.deferred_process = local.current_process.take();
        local.current_process = next_process;
        local.current_thread = Some(next);

        *ctx = next_context;
    }
}
