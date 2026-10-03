use crate::cpu::CpuIdentity;
use core::{arch::asm, ptr::NonNull};

pub fn current_cpu_identity() -> CpuIdentity {
    let value: u64;

    unsafe {
        asm!("mrs {value}, mpidr_el1", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };

    let affinity = ((value >> 32) & 0xff) << 24
        | ((value >> 16) & 0xff) << 16
        | ((value >> 8) & 0xff) << 8
        | (value & 0xff);

    CpuIdentity::new(affinity)
}

pub fn read_cpu_local<T>() -> *mut T {
    let value: u64;

    unsafe {
        asm!("mrs {value}, tpidr_el1", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };

    value as *mut T
}

pub unsafe fn write_cpu_local<T>(ptr: NonNull<T>) {
    unsafe { asm!("msr tpidr_el1, {0}", in(reg) ptr.as_ptr(), options(nostack, preserves_flags)) }
}
