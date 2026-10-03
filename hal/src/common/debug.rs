/// exit the execution environment with a status code
///
/// safety:
/// the execution environment must handle the platform's debug exit operation
pub unsafe fn exit(status: usize) -> ! {
    unsafe { crate::arch::debug::debug_exit(status) }
}
