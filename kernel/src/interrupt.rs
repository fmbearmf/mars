use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

use alloc::boxed::Box;
use klib::interrupt::InterruptController;

static mut INTERRUPT_CONTROLLER: MaybeUninit<Box<dyn InterruptController>> = MaybeUninit::uninit();
static CONTROLLER_STATUS: AtomicBool = AtomicBool::new(false);

pub fn set_interrupt_controller(imp: Box<dyn InterruptController>) {
    assert_eq!(CONTROLLER_STATUS.load(Ordering::Acquire), false);

    // safety: static mut is fine. data races are a non-issue since this is called once on one core.
    unsafe { INTERRUPT_CONTROLLER = MaybeUninit::new(imp) };
    CONTROLLER_STATUS.store(true, Ordering::Release);
}

pub fn get_interrupt_controller() -> &'static dyn InterruptController {
    assert_eq!(CONTROLLER_STATUS.load(Ordering::Acquire), true);

    #[allow(static_mut_refs)]
    // safety: nothing else can write to CONTROLLER_STATUS, so this is certainly initialized
    unsafe {
        INTERRUPT_CONTROLLER.assume_init_ref().as_ref()
    }
}
