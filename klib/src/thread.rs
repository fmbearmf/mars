use core::{fmt::Debug, range::Range};

use crate::pm::page::mapper::AddressTranslator;

use super::{context::RegisterFile, process::Process, sync::RwLock};

use aarch64_cpu::registers::SPSR_EL1;
use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
};
use derivative::Derivative;
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

#[derive(Derivative)]
#[derivative(Debug)]
struct ThreadInner<'a> {
    thread_id: ThreadId,
    state: ThreadState,
    priority: u8,
    kernel_sp: u64,
    stack: Option<Box<[u8]>>,
    process: Weak<Process<'a>>, // avoids a ref count

    #[derivative(Debug = "ignore")]
    translator: &'a dyn AddressTranslator,
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
    pub fn new(
        thread_id: ThreadId,
        process: &Arc<Process<'a>>,
        stack: Box<[u8]>,
        pc: usize,
        priority: u8,
        translator: &'a dyn AddressTranslator,
    ) -> Self {
        let stack_range = stack.as_ptr_range();
        let stack_top_va = stack_range.end;
        // let stack_top_pa = translator.dmap_to_phys(stack_top_va as *mut u8) as _;

        let ctx_ptr = stack_top_va as usize - size_of::<RegisterFile>();
        debug_assert_eq!(ctx_ptr, ctx_ptr & !0xF, "ctx_ptr not 16 aligned");
        let ctx = unsafe { &mut *(ctx_ptr as *mut RegisterFile) };

        ctx.registers = [0; 31];
        ctx.elr = pc as u64;
        ctx.spsr = SPSR_EL1::M::EL0t.value;
        ctx.sp = stack_top_va as u64;

        const SPSR: FieldValue<u64, SPSR_EL1::Register> = SPSR_EL1::M::EL0t;

        let inner = ThreadInner {
            thread_id,
            state: ThreadState::Ready,
            priority,
            kernel_sp: ctx_ptr as u64,
            stack: Some(stack),
            process: Arc::downgrade(process),
            translator,
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
