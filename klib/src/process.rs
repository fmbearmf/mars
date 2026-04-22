use core::fmt::Debug;

use crate::pm::page::mapper::AddressTranslator;

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
struct ProcessInner<'a> {
    process_id: ProcessId,
    state: ProcessState,
    address_space: AddressSpace<'a>,
    threads: Vec<Arc<Thread<'a>>>,
    parent: Option<Weak<Process<'a>>>,
}

#[derive(Clone)]
pub struct Process<'a> {
    inner: Arc<RwLock<ProcessInner<'a>>>,
}

impl Debug for Process<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.inner.read();
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
            parent: parent.map(Arc::downgrade),
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn add_thread(&self, thread: Arc<Thread<'a>>) {
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
        F: FnOnce(&AddressSpace) -> R,
    {
        let guard = self.inner.read();

        f(&guard.address_space)
    }

    pub fn with_threads<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&[Arc<Thread<'a>>]) -> R,
    {
        let guard = self.inner.read();

        f(guard.threads.as_ref())
    }

    pub fn with_threads_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut [Arc<Thread<'a>>]) -> R,
    {
        let mut guard = self.inner.write();

        f(guard.threads.as_mut())
    }

    pub fn process_id(&self) -> ProcessId {
        self.inner.read().process_id
    }
}
