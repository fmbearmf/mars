use core::{
    arch::asm,
    sync::atomic::{Ordering, compiler_fence},
};

const IRQ_MASK: u64 = 1 << 7;
const FIQ_MASK: u64 = 1 << 6;
const INTERRUPT_MASKS: u64 = IRQ_MASK | FIQ_MASK;

#[derive(Clone, Copy)]
pub(crate) struct InterruptState(u8);

pub(crate) fn mask() -> InterruptState {
    // wow
    compiler_fence(Ordering::SeqCst);

    let daif: u64;

    unsafe {
        asm!("mrs {daif}, daif", daif = out(reg) daif, options(nostack));
        asm!("msr daifset, #3", options(nostack));
    }

    compiler_fence(Ordering::SeqCst);

    InterruptState((daif & INTERRUPT_MASKS) as u8)
}

fn restore_masks(saved: InterruptState) {
    let masked = u64::from(saved.0) & INTERRUPT_MASKS;

    unsafe {
        match masked {
            0 => asm!("msr daifclr, #3", options(nostack)),
            IRQ_MASK => asm!("msr daifset, #2", "msr daifclr, #1", options(nostack)),
            FIQ_MASK => asm!("msr daifset, #1", "msr daifclr, #2", options(nostack)),
            _ => asm!("msr daifset, #3", options(nostack)),
        }
    }
}

pub(crate) fn restore(saved: InterruptState) {
    compiler_fence(Ordering::SeqCst);
    restore_masks(saved);
    compiler_fence(Ordering::SeqCst);
}

pub(crate) fn enable() {
    compiler_fence(Ordering::SeqCst);

    unsafe {
        asm!("msr daifclr, #3", options(nostack));
    }

    compiler_fence(Ordering::SeqCst);
}

pub(crate) fn is_enabled() -> bool {
    let daif: u64;

    unsafe {
        asm!("mrs {daif}, daif", daif = out(reg) daif, options(nostack));
    }

    daif & INTERRUPT_MASKS != INTERRUPT_MASKS
}
