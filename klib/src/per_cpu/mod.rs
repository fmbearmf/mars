use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize};

use alloc::vec::Vec;

use crate::cpu_interface::CpuIdLogical;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static REGISTRY_PTR: AtomicPtr<PerCpuData> = AtomicPtr::new(core::ptr::null_mut());
static REGISTRY_LEN: AtomicUsize = AtomicUsize::new(0);

/// TODO: make this data-oriented (to avoid false sharing with lots of CPUs sitting adjacent)
#[repr(C, align(64))]
pub struct PerCpuData {
    pub id: CpuIdLogical,
    /// for bootstrap only. owning core must set to true before BSP can continue.
    pub ready: AtomicBool,
    pub exception_depth: AtomicUsize,
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
        assert!(
            INITIALIZED
                .compare_exchange(
                    false,
                    true,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Acquire,
                )
                .is_ok(),
            "cpu registry already initialized"
        );

        let mut cpus = Vec::with_capacity(cores);
        for i in 0..cores {
            cpus.push(PerCpuData {
                id: CpuIdLogical::new(i as _),
                ready: AtomicBool::new(false),
                exception_depth: AtomicUsize::new(0),
            });
        }

        let leaked: &'static mut [PerCpuData] = cpus.leak();

        REGISTRY_LEN.store(leaked.len(), core::sync::atomic::Ordering::Relaxed);
        REGISTRY_PTR.store(leaked.as_mut_ptr(), core::sync::atomic::Ordering::Release);
    }

    pub fn all() -> &'static [PerCpuData] {
        // acquire the slice and its length together
        let ptr = REGISTRY_PTR.load(core::sync::atomic::Ordering::Acquire);
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
        assert!(hal::cpu::read_cpu_local::<PerCpuData>().is_null());

        let pcpu = Self::get(id).ok_or(())?;
        // the registry leaks its data and exposes only shared access
        unsafe { hal::cpu::write_cpu_local(core::ptr::NonNull::from(pcpu)) };

        Ok(())
    }

    /// return this cpu's registered data when cpu-local storage is available
    pub fn try_local() -> Option<&'static PerCpuData> {
        let ptr = hal::cpu::read_cpu_local::<PerCpuData>();
        if ptr.is_null() {
            return None;
        }

        // safety: registration stores a pointer to leaked registry data in this cpu's local slot
        Some(unsafe { &*ptr })
    }

    /// return this cpu's registered data
    pub fn local() -> &'static PerCpuData {
        Self::try_local().expect("cpu-local data is not registered")
    }
}
