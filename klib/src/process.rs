use core::fmt::Debug;

use super::{
    pm::page::mapper::TableAllocator,
    sync::RwLock,
    thread::{Thread, ThreadId},
    vm::{page_allocator::PhysicalPageAllocator, user::address_space::AddressSpace},
};

extern crate alloc;

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
struct ProcessInner<'a, A: TableAllocator, P: PhysicalPageAllocator> {
    process_id: ProcessId,
    state: ProcessState,
    address_space: AddressSpace<'a, A, P>,
    threads: Vec<Arc<Thread<'a, A, P>>>,
    parent: Option<Weak<Process<'a, A, P>>>,
}

#[derive(Clone)]
pub struct Process<'a, A: TableAllocator, P: PhysicalPageAllocator> {
    inner: Arc<RwLock<ProcessInner<'a, A, P>>>,
}

impl<'a, A: TableAllocator + Debug, P: PhysicalPageAllocator + Debug> Debug for Process<'a, A, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.inner.read();
        f.debug_tuple("Process").field(&*guard).finish()
    }
}

impl<'a, A: TableAllocator, P: PhysicalPageAllocator> Process<'a, A, P> {
    pub fn new(
        process_id: ProcessId,
        address_space: AddressSpace<'a, A, P>,
        parent: Option<&Arc<Process<'a, A, P>>>,
    ) -> Self {
        let inner = ProcessInner {
            process_id,
            state: ProcessState::Normal,
            address_space,
            threads: Vec::new(),
            parent: parent.map(Arc::downgrade),
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn add_thread(&self, thread: Arc<Thread<'a, A, P>>) {
        let mut guard = self.inner.write();
        guard.threads.push(thread);
    }

    pub fn remove_thread(&self, thread_id: ThreadId) {
        let mut guard = self.inner.write();
        guard.threads.retain(|t| t.thread_id() != thread_id);
    }

    pub fn set_state(&self, state: ProcessState) {
        self.inner.write().state = state;
    }

    pub fn get_state(&self) -> ProcessState {
        self.inner.read().state
    }

    pub fn with_address_space<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&AddressSpace<A, P>) -> R,
    {
        let guard = self.inner.read();

        f(&guard.address_space)
    }

    pub fn with_threads<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&[Arc<Thread<'a, A, P>>]) -> R,
    {
        let guard = self.inner.read();

        f(guard.threads.as_ref())
    }

    pub fn with_threads_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut [Arc<Thread<'a, A, P>>]) -> R,
    {
        let mut guard = self.inner.write();

        f(guard.threads.as_mut())
    }

    pub fn process_id(&self) -> ProcessId {
        self.inner.read().process_id
    }
}
