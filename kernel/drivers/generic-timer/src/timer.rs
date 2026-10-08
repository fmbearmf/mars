use atomic_refcell::AtomicRefCell;
use core::time::Duration;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TimerError {
    DurationTooLarge,
    CouldntBorrow,
}

const TIMER_DURATION: Duration = Duration::from_millis(100);

pub static TIMER: Timer = Timer::new();

pub fn init_timer() {
    TIMER.disarm();
    TIMER.set_masked(false);
}

pub fn timer_disarm() {
    TIMER.disarm();
}

pub fn timer_rearm() {
    TIMER.set_masked(false);
    TIMER.enable();
}

pub fn timer_schedule() {
    _ = TIMER.arm_after(TIMER_DURATION);
}

#[derive(Debug)]
pub struct Timer {
    timer_irq: AtomicRefCell<Option<u8>>,
}

impl Timer {
    pub const fn new() -> Self {
        Self {
            timer_irq: AtomicRefCell::new(None),
        }
    }

    pub fn set_irq(&self, irq: u8) -> Result<(), TimerError> {
        self.timer_irq
            .try_borrow_mut()
            .map_err(|_| TimerError::CouldntBorrow)?
            .replace(irq);

        Ok(())
    }

    pub fn get_irq(&self) -> Result<Option<u8>, TimerError> {
        Ok(*self
            .timer_irq
            .try_borrow()
            .map_err(|_| TimerError::CouldntBorrow)?)
    }

    pub fn freq_hz(&self) -> u64 {
        hal::timer::frequency_hz()
    }

    pub fn counter(&self) -> u64 {
        hal::timer::counter()
    }

    pub fn enabled(&self) -> bool {
        hal::timer::enabled()
    }

    pub fn masked(&self) -> bool {
        hal::timer::masked()
    }

    pub fn pending(&self) -> bool {
        hal::timer::pending()
    }

    pub fn enable(&self) {
        hal::timer::enable()
    }

    pub fn disable(&self) {
        hal::timer::disable()
    }

    pub fn set_masked(&self, masked: bool) {
        hal::timer::set_masked(masked)
    }

    pub fn set_compare(&self, value: u64) {
        hal::timer::set_deadline(value)
    }

    pub fn compare(&self) -> u64 {
        hal::timer::deadline()
    }

    pub fn arm_after(&self, duration: Duration) -> Result<(), TimerError> {
        let ticks = hal::timer::ticks_for(duration).ok_or(TimerError::DurationTooLarge)?;
        let deadline = self.counter().wrapping_add(ticks);

        self.set_compare(deadline);
        self.enable();

        Ok(())
    }

    pub fn wait(&self, deadline: u64) {
        hal::timer::wait_until(deadline)
    }

    pub fn sleep(&self, duration: Duration) -> Result<(), TimerError> {
        hal::timer::delay(duration)
            .then_some(())
            .ok_or(TimerError::DurationTooLarge)
    }

    pub fn disarm(&self) {
        self.set_masked(true);
        self.set_compare(u64::MAX);
        self.disable();
    }
}
