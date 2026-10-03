use core::ptr::NonNull;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct CpuIdentity(u64);

impl CpuIdentity {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

pub fn current_cpu_identity() -> CpuIdentity {
    crate::arch::cpu::current_cpu_identity()
}

pub fn read_cpu_local<T>() -> *mut T {
    crate::arch::cpu::read_cpu_local()
}

/// install this processor's cpu-local pointer
///
/// safety:
/// initialize the pointee and keep it live for every later access
/// synchronize shared data and preserve the installed pointee's type
pub unsafe fn write_cpu_local<T>(ptr: NonNull<T>) {
    unsafe { crate::arch::cpu::write_cpu_local(ptr) }
}
