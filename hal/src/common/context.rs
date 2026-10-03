#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct Context(crate::arch::context::Context);

impl core::fmt::Debug for Context {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&self.0, f)
    }
}

impl Context {
    pub(crate) fn raw_mut(&mut self) -> &mut crate::arch::context::Context {
        &mut self.0
    }

    pub const fn stack_alignment() -> usize {
        crate::arch::context::stack_alignment()
    }

    /// safety:
    /// supply a c-abi kernel entry that takes usize and never returns
    /// align stack_top to stack_alignment() and keep the stack mapped and live during execution
    pub unsafe fn kernel(entry: usize, stack_top: usize) -> Self {
        Self(unsafe { crate::arch::context::kernel(entry, stack_top) })
    }

    /// safety:
    /// supply a c-abi user entry that takes usize and never returns
    /// align stack_top to stack_alignment() and keep the stack and entry mapped and live
    /// the backend must support returning to user execution
    pub unsafe fn user(entry: usize, stack_top: usize) -> Self {
        Self(unsafe { crate::arch::context::user(entry, stack_top) })
    }

    /// set the first entry argument before this context runs
    ///
    /// safety:
    /// only use on a context before its first run, not some arbitrary suspended context
    pub unsafe fn set_argument(&mut self, argument: usize) {
        crate::arch::context::set_argument(&mut self.0, argument);
    }
}

/// request a reschedule
///
/// safety:
/// an exception handler must be installed that recognizes the reschedule request
pub unsafe fn request_reschedule() {
    unsafe { crate::arch::context::request_reschedule() }
}
