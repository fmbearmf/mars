pub trait LocalInterruptController: Send + Sync {
    fn initialize(&self);
    fn acknowledge(&self) -> Option<u32>;
    fn complete(&self, interrupt_id: u32);
    fn enable(&self);
    fn disable(&self);
    fn set_priority_limit(&self, limit: u8);
}

#[derive(Clone, Copy, Debug)]
pub struct PlatformLocalInterruptController;

impl LocalInterruptController for PlatformLocalInterruptController {
    fn initialize(&self) {
        crate::arch::local_interrupt::initialize();
    }
    fn acknowledge(&self) -> Option<u32> {
        crate::arch::local_interrupt::acknowledge()
    }
    fn complete(&self, interrupt_id: u32) {
        crate::arch::local_interrupt::complete(interrupt_id)
    }
    fn enable(&self) {
        crate::arch::local_interrupt::enable()
    }
    fn disable(&self) {
        crate::arch::local_interrupt::disable()
    }
    fn set_priority_limit(&self, limit: u8) {
        crate::arch::local_interrupt::set_priority_limit(limit)
    }
}
