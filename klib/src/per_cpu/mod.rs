use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize};

use aarch64_cpu::registers::{Readable, TPIDR_EL1};
use alloc::vec::Vec;
use atomic_refcell::AtomicRefCell;

use crate::interrupt::{GicdRegistersN, GicrRegisters};

static REGISTRY_PTR: AtomicPtr<PerCpuData> = AtomicPtr::new(core::ptr::null_mut());
static REGISTRY_LEN: AtomicUsize = AtomicUsize::new(0);

#[repr(C, align(64))]
pub struct PerCpuData {
    pub id: usize,
    pub redistributor: AtomicRefCell<Option<&'static mut GicrRegisters>>,
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
        assert_ne!(
            REGISTRY_PTR.load(core::sync::atomic::Ordering::Acquire),
            core::ptr::null_mut()
        );

        let mut cpus = Vec::with_capacity(cores);
        for i in 0..cores {
            cpus.push(PerCpuData {
                id: i,
                redistributor: AtomicRefCell::new(None),
                timer_irq: AtomicU8::new(0),
            });
        }

        let leaked: &'static mut [PerCpuData] = cpus.leak();

        REGISTRY_PTR.store(leaked.as_mut_ptr(), core::sync::atomic::Ordering::Release);
        REGISTRY_LEN.store(leaked.len(), core::sync::atomic::Ordering::Release);
    }

    pub fn all() -> &'static [PerCpuData] {
        // a store should only be ran once (at boot),
        // and this path might theoretically be hot,
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

    /// the assumption is that the caller has an initialized TPIDR_EL1
    pub fn local() -> &'static PerCpuData {
        let ptr = TPIDR_EL1.get() as *const PerCpuData;
        unsafe { &*ptr }
    }
}
