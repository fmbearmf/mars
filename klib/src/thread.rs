use core::{fmt::Debug, range::Range};

use crate::pm::page::mapper::AddressTranslator;

use super::{
    context::RegisterFile, pm::page::mapper::TableAllocator, process::Process, sync::RwLock,
    vm::page_allocator::PhysicalPageAllocator,
};

extern crate alloc;

use aarch64_cpu::registers::SPSR_EL1;
use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
};
use tock_registers::fields::FieldValue;

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
struct ThreadInner<'a, T: TableAllocator, P: PhysicalPageAllocator, A: AddressTranslator> {
    thread_id: ThreadId,
    state: ThreadState,
    priority: u8,
    ctx: RegisterFile,
    stack: Option<Box<[u8]>>,
    process: Weak<Process<'a, T, P, A>>, // avoids a ref count
}

#[derive(Clone)]
pub struct Thread<'a, T: TableAllocator, P: PhysicalPageAllocator, A: AddressTranslator> {
    inner: Arc<RwLock<ThreadInner<'a, T, P, A>>>,
}

impl<'a, T: TableAllocator + Debug, P: PhysicalPageAllocator + Debug, A: AddressTranslator + Debug>
    Debug for Thread<'a, T, P, A>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let guard = self.inner.read();
        f.debug_tuple("Thread").field(&*guard).finish()
    }
}

impl<'a, T: TableAllocator, P: PhysicalPageAllocator, A: AddressTranslator> Thread<'a, T, P, A> {
    pub fn new(
        thread_id: ThreadId,
        process: &Arc<Process<'a, T, P, A>>,
        stack: Box<[u8]>,
        pc: usize,
        priority: u8,
    ) -> Self {
        let stack_range = stack.as_ptr_range();
        let stack_top_va = stack_range.end;
        let stack_top_pa = A::dmap_to_phys(stack_top_va as *mut u8);

        const SPSR: FieldValue<u64, SPSR_EL1::Register> = SPSR_EL1::M::EL0t;

        let inner = ThreadInner {
            thread_id,
            state: ThreadState::Ready,
            priority,
            ctx: RegisterFile {
                registers: [0; 31],
                sp: stack_top_pa,
                spsr: SPSR.value,
                elr: pc,
            },
            stack: Some(stack),
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

    pub fn process(&self) -> Option<Arc<Process<'a, T, P, A>>> {
        self.inner.read().process.upgrade()
    }

    pub fn thread_id(&self) -> ThreadId {
        self.inner.read().thread_id
    }
}
