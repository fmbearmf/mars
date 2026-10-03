use core::marker::PhantomData;

use crate::arch::interrupt::InterruptState;

/// mask local maskable interrupts, restore their prior state on Drop
pub struct InterruptGuard {
    saved: InterruptState,
    _not_send_sync: PhantomData<*mut ()>,
}

impl InterruptGuard {
    pub fn new() -> Self {
        Self {
            saved: crate::arch::interrupt::mask(),
            _not_send_sync: PhantomData,
        }
    }

    /// enable local maskable interrupts.
    ///
    /// safety:
    /// enabling asynchronous exceptions must be valid in the current state of execution
    pub unsafe fn enable() {
        crate::arch::interrupt::enable();
    }

    pub fn disable() {
        crate::arch::interrupt::mask();
    }

    /// whether any maskable interrupt class is enabled
    pub fn is_enabled() -> bool {
        crate::arch::interrupt::is_enabled()
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        crate::arch::interrupt::restore(self.saved);
    }
}
