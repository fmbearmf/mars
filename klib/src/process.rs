use core::fmt::Debug;

use super::{
    sync::UnfairSpinlock,
    thread::{Thread, ThreadId},
    vm::user::address_space::AddressSpace,
};

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

pub type ProcessId = u32;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessState {
    Normal,
    Zombie,
}

#[derive(Debug)]
struct ProcessInner<'a> {
    process_id: ProcessId,
    state: ProcessState,
    address_space: AddressSpace<'a>,
    threads: Vec<Arc<Thread<'a>>>,
    _parent: Option<Weak<Process<'a>>>,
}

#[derive(Clone)]
pub struct Process<'a> {
    inner: Arc<UnfairSpinlock<ProcessInner<'a>>>,
}

impl Debug for Process<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.inner.lock();
        f.debug_tuple("Process").field(&*guard).finish()
    }
}

impl<'a> Process<'a> {
    pub fn new(
        process_id: ProcessId,
        address_space: AddressSpace<'a>,
        parent: Option<&Arc<Process<'a>>>,
    ) -> Self {
        let inner = ProcessInner {
            process_id,
            state: ProcessState::Normal,
            address_space,
            threads: Vec::new(),
            _parent: parent.map(Arc::downgrade),
        };

        Self {
            inner: Arc::new(UnfairSpinlock::new(inner)),
        }
    }

    pub fn add_thread(&self, thread: Arc<Thread<'a>>) {
        let mut guard = self.inner.lock();
        guard.threads.push(thread);
    }

    pub fn remove_thread(&self, thread_id: ThreadId) {
        let mut guard = self.inner.lock();
        guard.threads.retain(|t| t.thread_id() != thread_id);
    }

    pub fn set_state(&self, state: ProcessState) {
        self.inner.lock().state = state;
    }

    pub fn get_state(&self) -> ProcessState {
        self.inner.lock().state
    }

    /// the callback runs while the process lock is held and must not yield, block, or reenter this process
    pub fn with_address_space<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&AddressSpace) -> R,
    {
        let guard = self.inner.lock();

        f(&guard.address_space)
    }

    /// the callback runs while the process lock is held and must not yield, block, or reenter this process
    pub fn with_threads<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&[Arc<Thread<'a>>]) -> R,
    {
        let guard = self.inner.lock();

        f(guard.threads.as_ref())
    }

    /// the callback runs while the process lock is held and must not yield, block, or reenter this process
    pub fn with_threads_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut [Arc<Thread<'a>>]) -> R,
    {
        let mut guard = self.inner.lock();

        f(guard.threads.as_mut())
    }

    pub fn process_id(&self) -> ProcessId {
        self.inner.lock().process_id
    }
}
