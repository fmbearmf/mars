use super::{
    context::RegisterFile,
    pm::page::mapper::TableAllocator,
    process::Process,
    sync::{Mutex, RwLock},
    vm::page_allocator::PhysicalPageAllocator,
};

extern crate alloc;

use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
};

pub type ThreadId = u32;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadState {
    Running,
    Ready,
    Blocked,
    Dead,
}

#[derive(Debug)]
struct ThreadInner<'a, A: TableAllocator, P: PhysicalPageAllocator> {
    thread_id: ThreadId,
    state: ThreadState,
    priority: u8,
    ctx: RegisterFile,
    stack: Option<Box<[u8]>>,
    process: Weak<Process<'a, A, P>>, // avoids a ref count
}

#[derive(Debug, Clone)]
pub struct Thread<'a, A: TableAllocator, P: PhysicalPageAllocator> {
    inner: Arc<RwLock<ThreadInner<'a, A, P>>>,
}

impl<'a, A: TableAllocator, P: PhysicalPageAllocator> Thread<'a, A, P> {
    pub fn new(
        thread_id: ThreadId,
        process: &Arc<Process<'a, A, P>>,
        stack_size: usize,
        pc: u64,
        priority: u8,
    ) -> Self {
        let stack = if stack_size > 0 {
            let stack = alloc::vec![0u8; stack_size].into_boxed_slice();
            Some(stack)
        } else {
            None
        };

        let inner = ThreadInner {
            thread_id,
            state: ThreadState::Ready,
            priority,
            ctx: RegisterFile {
                registers: [0; 31],
                sp: stack
                    .as_ref()
                    .map_or(0, |s| s.as_ptr() as u64 + s.len() as u64),
                spsr: 0,
                elr: 0,
            },
            stack,
            process: Arc::downgrade(process),
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
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
        f(&mut guard.ctx)
    }

    pub fn with_ctx<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&RegisterFile) -> R,
    {
        let guard = self.inner.read();
        f(&guard.ctx)
    }

    pub fn process(&self) -> Option<Arc<Process<'a, A, P>>> {
        self.inner.read().process.upgrade()
    }

    pub fn thread_id(&self) -> ThreadId {
        self.inner.read().thread_id
    }
}
