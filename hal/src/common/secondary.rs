/// state consumed by the processor startup trampoline
#[repr(transparent)]
pub struct SecondaryBootState(crate::arch::secondary::SecondaryBootState);

/// capture the current processor state for a secondary processor
///
/// safety:
/// keep the state and stack alive until the processor publishes readiness
/// supply a c-abi entry that takes u32 and never returns
/// map the trampoline, state, stack, and entry during startup
pub unsafe fn prepare(
    stack_top: usize,
    entry: unsafe extern "C" fn(u32) -> !,
    argument: u32,
) -> SecondaryBootState {
    SecondaryBootState(unsafe { crate::arch::secondary::prepare(stack_top, entry, argument) })
}

/// return the virtual address of the startup trampoline
pub fn entry_address() -> usize {
    crate::arch::secondary::entry_address()
}
