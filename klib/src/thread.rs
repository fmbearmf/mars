use core::{fmt::Debug, range::Range, sync::atomic::Atomic};

use crate::{pm::page::mapper::AddressTranslator, stack::Stack, sync::FairSpinlock};

use super::{context::RegisterFile, process::Process, sync::RwLock};

use aarch64_cpu::registers::SPSR_EL1;
use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};
use derivative::Derivative;
use tock_registers::fields::FieldValue;

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
    kernel_sp: u64,
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
        pc: usize,
        priority: u8,
        spsr_value: u64,
        // translator: &'a dyn AddressTranslator,
    ) -> Self {
        let stack_range = stack.as_ptr_range();
        let stack_top_va = stack_range.end;
        // let stack_top_pa = translator.dmap_to_phys(stack_top_va as *mut u8) as _;

        let ctx_ptr = stack_top_va as usize - size_of::<RegisterFile>();
        debug_assert_eq!(ctx_ptr, ctx_ptr & !0xF, "ctx_ptr not 16 aligned");
        let ctx = unsafe { &mut *(ctx_ptr as *mut RegisterFile) };

        ctx.registers = [0; 31];
        ctx.elr = pc as u64;
        ctx.spsr = spsr_value;
        ctx.sp = stack_top_va as u64;

        let thread_id = THREAD_ID_ALLOC.alloc();

        let inner = ThreadInner {
            thread_id,
            state: ThreadState::Ready,
            priority,
            kernel_sp: ctx_ptr as u64,
            stack: Some(stack),
            process,
            is_kernel,
            // translator,
        };

        Self {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    pub fn new(
        process: &Arc<Process<'a>>,
        stack: Stack,
        pc: usize,
        priority: u8,
        // translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self::new_inner(
            Arc::downgrade(process),
            false,
            stack,
            pc,
            priority,
            SPSR_EL1::M::EL0t.value,
            // translator,
        )
    }

    pub fn new_kernel(
        stack: Stack,
        entry: *const (),
        priority: u8,
        // translator: &'a dyn AddressTranslator,
    ) -> Self {
        Self::new_inner(
            Weak::new(),
            true,
            stack,
            entry as _,
            priority,
            SPSR_EL1::M::EL1h.value,
            // translator,
        )
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
        let guard = self.inner.write();

        assert_ne!(
            guard.state,
            ThreadState::Running,
            "can't access context of a running thread"
        );

        let ctx_ptr = guard.kernel_sp as *mut RegisterFile;
        unsafe { f(&mut *ctx_ptr) }
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

        let ctx_ptr = guard.kernel_sp as *const RegisterFile;

        unsafe { f(&*ctx_ptr) }
    }

    pub fn process(&self) -> Option<Arc<Process<'a>>> {
        self.inner.read().process.upgrade()
    }

    pub fn thread_id(&self) -> ThreadId {
        self.inner.read().thread_id
    }
}
