use core::time::Duration;

#[derive(Debug, Copy, Clone)]
pub struct TimerCapability {
    pub is_virtual: bool,
    pub is_hypervisor: bool,
    pub is_secure: bool,
}

pub fn timer_filter() -> impl Fn(&TimerCapability) -> bool {
    crate::arch::timer::timer_filter()
}

pub fn frequency_hz() -> u64 {
    crate::arch::timer::timer_frequency()
}

pub fn counter() -> u64 {
    crate::arch::timer::timer_counter()
}

/// read the timer counter with instruction and compiler ordering around the read
///
/// preceding and following instructions are separated from the counter read by
/// the architecture's instruction synchronization barrier.
/// the compiler is also prevented from moving operations across the read.
pub fn ordered_counter() -> u64 {
    crate::arch::timer::ordered_counter()
}

pub fn deadline() -> u64 {
    crate::arch::timer::timer_deadline()
}

pub fn enabled() -> bool {
    crate::arch::timer::timer_enabled()
}

pub fn masked() -> bool {
    crate::arch::timer::timer_masked()
}

pub fn pending() -> bool {
    crate::arch::timer::timer_pending()
}

pub fn set_deadline(value: u64) {
    crate::arch::timer::timer_set_deadline(value)
}

pub fn enable() {
    crate::arch::timer::timer_enable()
}

pub fn disable() {
    crate::arch::timer::timer_disable()
}

pub fn set_masked(value: bool) {
    crate::arch::timer::timer_set_masked(value)
}

pub fn ticks_for(duration: Duration) -> Option<u64> {
    let frequency = u128::from(frequency_hz());
    if frequency == 0 {
        return None;
    }

    let ticks = u128::from(duration.as_secs()) * frequency
        + u128::from(duration.subsec_nanos()) * frequency / 1_000_000_000;

    (ticks < (1u128 << 63)).then_some(ticks as u64)
}

/// wait until the deadline, which must be fewer than 2^63 ticks ahead
pub fn wait_until(deadline: u64) {
    while (counter().wrapping_sub(deadline) as i64) < 0 {
        core::hint::spin_loop();
    }
}

pub fn delay(duration: Duration) -> bool {
    let Some(ticks) = ticks_for(duration) else {
        return false;
    };

    wait_until(counter().wrapping_add(ticks));

    true
}
