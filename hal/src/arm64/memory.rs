pub(crate) fn publish_to_device() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

pub(crate) fn acquire_from_device() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}
