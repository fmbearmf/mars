use core::fmt::Debug;

use crate::{stack::Stack, sync::FairSpinlock};

use super::{context::RegisterFile, process::Process, sync::RwLock};

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
    inner: Arc<RwLock<ThreadInner<'a>>>,
}

impl Debug for Thread<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.inner.read();
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
            priority,
            context,
            stack: Some(stack),
            process,
            is_kernel,
            // translator,
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
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
        self.inner.read().is_kernel
    }

    pub fn set_state(&self, state: ThreadState) {
        self.inner.write().state = state;
    }

    pub fn get_state(&self) -> ThreadState {
        self.inner.read().state
    }

    pub fn set_priority(&self, priority: u8) {
        self.inner.write().priority = priority;
    }

    pub fn with_ctx_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut RegisterFile) -> R,
    {
        let mut guard = self.inner.write();

        assert_ne!(
            guard.state,
            ThreadState::Running,
            "can't access context of a running thread"
        );

        f(&mut guard.context)
    }

    pub(crate) fn save_running_context(&self, context: RegisterFile) {
        self.inner.write().context = context;
    }

    pub fn with_ctx<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&RegisterFile) -> R,
    {
        let guard = self.inner.read();

        assert_ne!(
            guard.state,
            ThreadState::Running,
            "can't access context of a running thread"
        );

        f(&guard.context)
    }

    pub fn process(&self) -> Option<Arc<Process<'a>>> {
        self.inner.read().process.upgrade()
    }

    pub fn thread_id(&self) -> ThreadId {
        self.inner.read().thread_id
    }
}
