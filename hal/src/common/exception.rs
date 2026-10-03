#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Exception {
    Reschedule,
    Interrupt,
    Fault { user: bool },
    Fatal,
}

/// handle an exception and select the context to resume
///
/// safety:
/// retain the selected stack and mappings through exception return
/// never select a context that another processor can execute concurrently
pub type Handler = unsafe fn(Exception, &mut crate::context::Context);

/// install the process-wide exception handler
///
/// safety:
/// mask local interrupts and keep the handler's code mapped on every processor
pub unsafe fn install(handler: Handler) {
    unsafe { crate::arch::exception::install(handler) }
}

/// resume a saved execution context
///
/// safety:
/// own the selected execution state exclusively
/// retain its stack and mappings until execution stops
pub unsafe fn resume(context: &mut crate::context::Context) -> ! {
    unsafe { crate::arch::exception::resume(context) }
}

/// install this processor's exception vectors
///
/// safety:
/// vectors must remain mapped and local interrupts must be masked
pub unsafe fn install_vectors() {
    unsafe { crate::arch::exception::install_vectors() }
}
