use super::{
    sync::RwLock,
    thread::{Thread, ThreadId},
    vm::{
        TABLE_ENTRIES, TTable, VmError, map::Map as VmMap, mapper::TableAllocator,
        page_allocator::PhysicalPageAllocator,
    },
};

extern crate alloc;

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
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
    address_space: &'a mut TTable<TABLE_ENTRIES>,
    vm_map: VmMap,
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
        address_space: &'a mut TTable<TABLE_ENTRIES>,
        parent: Option<&Arc<Process<'a>>>,
    ) -> Self {
        let inner = ProcessInner {
            process_id,
            state: ProcessState::Normal,
            address_space,
            vm_map: VmMap::new(),
            threads: Vec::new(),
            parent: parent.map(Arc::downgrade),
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn mmap_anonymous<A: TableAllocator, P: PhysicalPageAllocator>(
        &self,
        va_hint: Option<usize>,
        size: usize,
        ap: AccessPermission,
        share: Shareability,
        uxn: bool,
        pxn: bool,
        attr_index: u64,
        table_alloc: &A,
        page_alloc: &P,
    ) -> Result<usize, VmError> {
        let mut guard = self.inner.write();
        let ProcessInner {
            process_id,
            state,
            address_space,
            vm_map,
            threads,
            parent,
        } = &mut *guard;

        vm_map.mmap_anonymous(
            address_space,
            va_hint,
            size,
            ap,
            share,
            uxn,
            pxn,
            attr_index,
            table_alloc,
            page_alloc,
        )
    }

    pub fn munmap<A: TableAllocator, P: PhysicalPageAllocator>(
        &self,
        va: usize,
        size: usize,
        table_alloc: &A,
        page_alloc: &P,
    ) -> Result<(), VmError> {
        let mut guard = self.inner.write();
        let ProcessInner {
            process_id,
            state,
            address_space,
            vm_map,
            threads,
            parent,
        } = &mut *guard;
        //let root = &mut *guard.address_space;

        vm_map.remove(address_space, va, size, table_alloc, page_alloc)
    }

    pub fn destroy<A: TableAllocator, P: PhysicalPageAllocator>(
        &self,
        table_alloc: &A,
        page_alloc: &P,
    ) -> Result<(), VmError> {
        let mut guard = self.inner.write();
        let ProcessInner {
            process_id,
            state,
            address_space,
            vm_map,
            threads,
            parent,
        } = &mut *guard;

        vm_map.clear(address_space, table_alloc, page_alloc)?;
        guard.state = ProcessState::Zombie;
        Ok(())
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

    pub fn with_address_space<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&TTable<TABLE_ENTRIES>) -> R,
    {
        let guard = self.inner.read();

        f(guard.address_space)
    }

    pub fn process_id(&self) -> ProcessId {
        self.inner.read().process_id
    }

    pub fn threads(&self) -> Vec<Arc<Thread<'a>>> {
        self.inner.read().threads.clone()
    }
}
