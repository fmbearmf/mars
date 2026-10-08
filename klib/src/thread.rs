use core::fmt::Debug;

use crate::{
    stack::Stack,
    sync::{FairSpinlock, UnfairSpinlock},
};

use super::{context::RegisterFile, process::Process};

use alloc::sync::{Arc, Weak};
use derivative::Derivative;

pub type ThreadId = u32;

struct ThreadIdPool {
    next_id: ThreadId,
}

struct ThreadIdAllocator(FairSpinlock<ThreadIdPool>);
impl ThreadIdAllocator {
    const fn new() -> Self {
        Self(FairSpinlock::new(ThreadIdPool { next_id: 0 }))
    }

    pub fn alloc(&self) -> ThreadId {
        let mut pool = self.0.lock();
        let id = pool.next_id;
        pool.next_id += 1;
        id
    }

    pub fn free(&self, id: ThreadId) {
        _ = id;
    }
}

static THREAD_ID_ALLOC: ThreadIdAllocator = ThreadIdAllocator::new();

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadState {
    Running,
    Ready,
    Blocked,
    Dead,
}

#[derive(Derivative)]
#[derivative(Debug)]
struct ThreadInner<'a> {
    thread_id: ThreadId,
    state: ThreadState,
    scheduled: bool,
    queued: bool,
    idle: bool,
    priority: u8,
    context: RegisterFile,
    stack: Option<Stack>,
    process: Weak<Process<'a>>, // avoids a ref count
    is_kernel: bool,
    // #[derivative(Debug = "ignore")]
    // translator: &'a dyn AddressTranslator,
}

#[derive(Clone)]
pub struct Thread<'a> {
    inner: Arc<UnfairSpinlock<ThreadInner<'a>>>,
}

impl Debug for Thread<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.inner.lock();
        f.debug_tuple("Thread").field(&*guard).finish()
    }
}

impl<'a> Thread<'a> {
    fn new_inner(
        process: Weak<Process<'a>>,
        is_kernel: bool,
        stack: Stack,
        context: RegisterFile,
        priority: u8,
        // translator: &'a dyn AddressTranslator,
    ) -> Self {
        let thread_id = THREAD_ID_ALLOC.alloc();

        let inner = ThreadInner {
            thread_id,
            state: ThreadState::Ready,
            scheduled: false,
            queued: false,
            idle: false,
            priority,
            context,
            stack: Some(stack),
            process,
            is_kernel,
            // translator,
        };

        Self {
            inner: Arc::new(UnfairSpinlock::new(inner)),
        }
    }

    /// safety
    /// `entry` must be a valid user entry point compatible with the ABI used to start it,
    /// and the process must retain all mappings and backing memory needed by the entry and stack
    pub unsafe fn new(
        process: &Arc<Process<'a>>,
        stack: Stack,
        entry: usize,
        priority: u8,
        // translator: &'a dyn AddressTranslator,
    ) -> Self {
        let stack_top = stack.top() as usize;
        assert_eq!(
            stack_top % hal::context::Context::stack_alignment(),
            0,
            "user stack top must satisfy the architecture stack alignment"
        );
        let context = unsafe { hal::context::Context::user(entry, stack_top) };
        Self::new_inner(Arc::downgrade(process), false, stack, context, priority)
    }

    pub fn new_kernel(
        stack: Stack,
        entry: extern "C" fn(usize) -> !,
        priority: u8,
        // translator: &'a dyn AddressTranslator,
    ) -> Self {
        let stack_top = stack.top() as usize;
        assert_eq!(
            stack_top % hal::context::Context::stack_alignment(),
            0,
            "kernel stack top must satisfy the architecture stack alignment"
        );
        let context = unsafe { hal::context::Context::kernel(entry as usize, stack_top) };
        Self::new_inner(Weak::new(), true, stack, context, priority)
    }

    pub fn is_kernel(&self) -> bool {
        self.inner.lock().is_kernel
    }

    pub(crate) fn set_state(&self, state: ThreadState) {
        let mut inner = self.inner.lock();

        assert!(
            !inner.idle || state != ThreadState::Blocked,
            "idle thread cannot block"
        );
        assert!(
            inner.scheduled,
            "only a scheduled thread may change its state"
        );

        inner.state = state;
    }

    pub(crate) fn schedule_fresh(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.idle || inner.state != ThreadState::Ready || inner.queued || inner.scheduled {
            return false;
        }

        inner.queued = true;

        true
    }

    pub(crate) fn claim_queued(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.state != ThreadState::Ready || !inner.queued || inner.scheduled {
            return false;
        }

        inner.queued = false;
        inner.scheduled = true;
        inner.state = ThreadState::Running;

        true
    }

    pub(crate) fn block_scheduled(&self) {
        let mut inner = self.inner.lock();

        assert!(
            inner.scheduled && !inner.idle,
            "only a scheduled non-idle thread may block"
        );

        inner.state = ThreadState::Blocked;
    }

    pub(crate) fn wake(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.state != ThreadState::Blocked {
            return false;
        }

        inner.state = ThreadState::Ready;

        if inner.scheduled || inner.queued {
            false
        } else {
            inner.queued = true;
            true
        }
    }

    pub(crate) fn finish_switch(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.idle {
            inner.state = ThreadState::Ready;
            return false;
        }

        inner.scheduled = false;
        match inner.state {
            ThreadState::Ready if !inner.queued => {
                inner.queued = true;

                true
            }
            _ => false,
        }
    }

    pub(crate) fn start_idle(&self) {
        let mut inner = self.inner.lock();

        inner.scheduled = true;
        inner.idle = true;
        inner.state = ThreadState::Running;
    }

    pub(crate) fn resume_idle(&self) {
        let mut inner = self.inner.lock();
        assert!(inner.idle && inner.scheduled);

        inner.state = ThreadState::Running;
    }

    pub(crate) fn context_for_switch(&self) -> RegisterFile {
        let inner = self.inner.lock();
        assert!(inner.scheduled && inner.state == ThreadState::Running);

        inner.context
    }

    pub fn get_state(&self) -> ThreadState {
        self.inner.lock().state
    }

    pub fn set_priority(&self, priority: u8) {
        self.inner.lock().priority = priority;
    }

    /// access the saved context while holding the thread lock
    ///
    /// callbacks must not yield or reenter this thread
    pub fn with_ctx_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut RegisterFile) -> R,
    {
        let mut guard = self.inner.lock();

        assert_ne!(
            guard.state,
            ThreadState::Running,
            "can't access context of a running thread"
        );

        f(&mut guard.context)
    }

    pub(crate) fn save_running_context(&self, context: RegisterFile) {
        self.inner.lock().context = context;
    }

    /// access the saved context while holding the thread lock
    ///
    /// callbacks must not yield or reenter this thread
    pub fn with_ctx<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&RegisterFile) -> R,
    {
        let guard = self.inner.lock();

        assert_ne!(
            guard.state,
            ThreadState::Running,
            "can't access context of a running thread"
        );

        f(&guard.context)
    }

    pub fn process(&self) -> Option<Arc<Process<'a>>> {
        self.inner.lock().process.upgrade()
    }

    pub fn thread_id(&self) -> ThreadId {
        self.inner.lock().thread_id
    }
}
