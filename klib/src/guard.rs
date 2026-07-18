use aarch64_cpu::registers::{DAIF, ReadWriteable, Readable, Writeable};

pub struct InterruptGuard {
    daif: u64,
}

impl InterruptGuard {
    pub fn new() -> InterruptGuard {
        let old = DAIF.get();
        DAIF.modify(DAIF::I::Masked + DAIF::F::Masked);

        InterruptGuard { daif: old }
    }

    /// enable interrupts
    pub fn enable() {
        DAIF.modify(DAIF::I::Unmasked + DAIF::F::Unmasked);
    }

    /// disable interrupts
    pub fn disable() {
        DAIF.modify(DAIF::I::Masked + DAIF::F::Masked);
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        DAIF.set(self.daif);
    }
}
