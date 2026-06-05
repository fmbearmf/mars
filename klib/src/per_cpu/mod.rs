use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize};

use aarch64_cpu::registers::{Readable, TPIDR_EL1, Writeable};
use alloc::{sync::Arc, vec::Vec};
use atomic_refcell::AtomicRefCell;

use crate::{
    cpu_interface::CpuIdLogical,
    interrupt::GicrRegisters,
    thread::{Thread, ThreadId},
};

static REGISTRY_PTR: AtomicPtr<PerCpuData> = AtomicPtr::new(core::ptr::null_mut());
static REGISTRY_LEN: AtomicUsize = AtomicUsize::new(0);

#[repr(C, align(64))]
pub struct PerCpuData {
    pub id: CpuIdLogical,
    pub current_thread: Option<ThreadId>,
    pub timer_irq: AtomicU8,
}

#[macro_export]
macro_rules! this_cpu {
    () => {
        ($crate::per_cpu::PerCpu::local())
    };
}

pub struct PerCpu;

impl PerCpu {
    pub fn init(cores: usize) {
        debug_assert_eq!(
            REGISTRY_PTR.load(core::sync::atomic::Ordering::Acquire),
            core::ptr::null_mut()
        );

        let mut cpus = Vec::with_capacity(cores);
        for i in 0..cores {
            cpus.push(PerCpuData {
                id: CpuIdLogical::new(i as _),
                current_thread: None,
                timer_irq: AtomicU8::new(0),
            });
        }

        let leaked: &'static mut [PerCpuData] = cpus.leak();

        REGISTRY_PTR.store(leaked.as_mut_ptr(), core::sync::atomic::Ordering::Release);
        REGISTRY_LEN.store(leaked.len(), core::sync::atomic::Ordering::Release);
    }

    pub fn all() -> &'static [PerCpuData] {
        // a store should only be ran once (at boot, on one core),
        // and this path could be hot,
        // so Relaxed is optimal
        let ptr = REGISTRY_PTR.load(core::sync::atomic::Ordering::Relaxed);
        let len = REGISTRY_LEN.load(core::sync::atomic::Ordering::Relaxed);

        if ptr.is_null() {
            return &[];
        }

        unsafe { core::slice::from_raw_parts(ptr, len) }
    }

    pub fn get(id: usize) -> Option<&'static PerCpuData> {
        Self::all().get(id)
    }

    pub fn register_local(id: usize) -> Result<(), ()> {
        debug_assert_eq!(TPIDR_EL1.get(), 0);

        let pcpu = Self::get(id).ok_or(())?;
        TPIDR_EL1.set(pcpu as *const _ as u64);

        Ok(())
    }

    /// the assumption is that the caller has an initialized TPIDR_EL1
    pub fn local() -> &'static PerCpuData {
        let ptr = TPIDR_EL1.get() as *const PerCpuData;
        debug_assert!(!ptr.is_null());
        unsafe { &*ptr }
    }
}
