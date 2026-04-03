use super::{
    sync::RwLock,
    thread::{Thread, ThreadId},
    vm::{TABLE_ENTRIES, TTable},
};

extern crate alloc;

use alloc::{
    boxed::Box,
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
    address_space: &'a TTable<TABLE_ENTRIES>,
    threads: Vec<Arc<Thread<'a>>>,
    parent: Option<Weak<Process<'a>>>,
}

#[derive(Debug, Clone)]
pub struct Process<'a> {
    inner: Arc<RwLock<ProcessInner<'a>>>,
}

impl<'a> Process<'a> {
    pub fn new(
        process_id: ProcessId,
        address_space: &'a TTable<TABLE_ENTRIES>,
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
        self.inner.write().threads.push(thread);
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

    pub fn address_space(&self) -> &TTable<TABLE_ENTRIES> {
        self.inner.read().address_space
    }

    pub fn process_id(&self) -> ProcessId {
        self.inner.read().process_id
    }

    pub fn threads(&self) -> Vec<Arc<Thread<'a>>> {
        self.inner.read().threads.clone()
    }
}
