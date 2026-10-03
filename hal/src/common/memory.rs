/// complete prior memory accesses before transferring ownership to a device
pub fn publish_to_device() {
    crate::arch::memory::publish_to_device()
}
/// complete device-visible accesses before the cpu consumes device results
pub fn acquire_from_device() {
    crate::arch::memory::acquire_from_device()
}
