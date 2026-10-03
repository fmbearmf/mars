//! hopefully this API works on non-ARM architectures :clueless:

/// prepare processor state after leaving firmware
///
/// safety:
/// run once with interrupts masked. identity-map the current code and stack
pub unsafe fn leave_firmware() {
    unsafe { crate::arch::boot::leave_firmware() }
}

/// initialize this processor before registering cpu-local storage
///
/// safety:
/// call before publishing this processor or borrowing cpu-local data
pub unsafe fn initialize_processor() {
    unsafe { crate::arch::boot::initialize_processor() }
}

/// enter the kernel with its bootstrap argument
///
/// safety:
/// map a c-abi entry that takes usize and never returns. keep its argument live
/// retain the current code and stack mappings until entry installs its stack
pub unsafe fn enter_kernel(entry: usize, argument: usize) -> ! {
    unsafe { crate::arch::boot::enter_kernel(entry, argument) }
}

pub fn stack_pointer() -> usize {
    crate::arch::boot::stack_pointer()
}

/// wait for an interrupt or a platform wakeup
/// callers must recheck their condition after returning
pub fn wait() {
    crate::arch::boot::wait()
}

/// bind the kernel entry to the platform's startup trampoline
#[macro_export]
macro_rules! kernel_entry {
    ($entry:path) => {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn hal_kernel_entry(argument: usize) -> ! {
            let entry: unsafe extern "C" fn(usize) -> ! = $entry;
            unsafe { entry(argument) }
        }
    };
}
