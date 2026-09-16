use core::time::Duration;

use aarch64_cpu::{
    asm::barrier::{self, isb},
    registers::{
        CNTFRQ_EL0, CNTP_CTL_EL0, CNTP_CVAL_EL0, CNTPCT_EL0, CNTV_CTL_EL0, CNTV_CVAL_EL0,
        CNTVCT_EL0, CurrentEL, Readable, Writeable,
    },
};
use atomic_refcell::AtomicRefCell;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TimerError {
    DurationTooLarge,
    CouldntBorrow,
}

pub static TIMER: Timer = Timer::new();
const TIMER_DURATION: Duration = Duration::from_millis(100);

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
        let mut timer_irq = self
            .timer_irq
            .try_borrow_mut()
            .map_err(|_| TimerError::CouldntBorrow)?;

        timer_irq.replace(irq);

        Ok(())
    }

    pub fn get_irq(&self) -> Result<Option<u8>, TimerError> {
        let timer_irq = self
            .timer_irq
            .try_borrow()
            .map_err(|_| TimerError::CouldntBorrow)?;

        Ok(*timer_irq)
    }

    #[inline]
    pub fn freq_hz(&self) -> u64 {
        read_cntfrq_el0()
    }

    #[inline]
    pub fn counter(&self) -> u64 {
        match CurrentEL.read(CurrentEL::EL) {
            2 => read_cntpct_el0(),
            _ => read_cntvct_el0(),
        }
    }

    // can't be bothered to write a `register_bitfields`
    #[inline]
    pub fn enabled(&self) -> bool {
        match CurrentEL.read(CurrentEL::EL) {
            2 => CNTP_CTL_EL0.matches_all(CNTP_CTL_EL0::ENABLE::SET),
            _ => CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::ENABLE::SET),
        }
    }

    #[inline]
    pub fn masked(&self) -> bool {
        match CurrentEL.read(CurrentEL::EL) {
            2 => CNTP_CTL_EL0.matches_all(CNTP_CTL_EL0::IMASK::SET),
            _ => CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::IMASK::SET),
        }
    }

    #[inline]
    pub fn pending(&self) -> bool {
        match CurrentEL.read(CurrentEL::EL) {
            2 => CNTP_CTL_EL0.matches_all(CNTP_CTL_EL0::ISTATUS::SET),
            _ => CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::ISTATUS::SET),
        }
    }

    #[inline]
    pub fn enable(&self) {
        match CurrentEL.read(CurrentEL::EL) {
            2 => CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::SET),
            _ => CNTV_CTL_EL0.write(CNTV_CTL_EL0::ENABLE::SET),
        }
        isb(barrier::SY);
    }

    #[inline]
    pub fn disable(&self) {
        match CurrentEL.read(CurrentEL::EL) {
            2 => CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::CLEAR),
            _ => CNTV_CTL_EL0.write(CNTV_CTL_EL0::ENABLE::CLEAR),
        }
        isb(barrier::SY);
    }

    #[inline]
    pub fn set_masked(&self, masked: bool) {
        match (CurrentEL.read(CurrentEL::EL), masked) {
            (2, true) => CNTP_CTL_EL0.write(CNTP_CTL_EL0::IMASK::SET),
            (2, false) => CNTP_CTL_EL0.write(CNTP_CTL_EL0::IMASK::CLEAR),
            (_, true) => CNTV_CTL_EL0.write(CNTV_CTL_EL0::IMASK::SET),
            (_, false) => CNTV_CTL_EL0.write(CNTV_CTL_EL0::IMASK::CLEAR),
        };

        isb(barrier::SY);
    }

    #[inline]
    pub fn set_compare(&self, cval: u64) {
        match CurrentEL.read(CurrentEL::EL) {
            2 => write_cntp_cval_el0(cval),
            _ => write_cntv_cval_el0(cval),
        }
        isb(barrier::SY);
    }

    #[inline]
    pub fn compare(&self) -> u64 {
        match CurrentEL.read(CurrentEL::EL) {
            2 => read_cntp_cval_el0(),
            _ => read_cntv_cval_el0(),
        }
    }

    pub fn arm_after(&self, delta: Duration) -> Result<(), TimerError> {
        let ticks = to_ticks(delta, self.freq_hz()).ok_or(TimerError::DurationTooLarge)?;
        let now = self.counter();
        let deadline = now.checked_add(ticks).ok_or(TimerError::DurationTooLarge)?;

        self.set_compare(deadline);
        self.enable();

        Ok(())
    }

    pub fn wait(&self, deadline: u64) {
        while (self.counter().wrapping_sub(deadline) as i64) < 0 {
            core::hint::spin_loop();
        }
    }

    pub fn sleep(&self, duration: Duration) -> Result<(), TimerError> {
        let ticks = to_ticks(duration, self.freq_hz()).ok_or(TimerError::DurationTooLarge)?;
        let deadline = self
            .counter()
            .checked_add(ticks)
            .ok_or(TimerError::DurationTooLarge)?;

        self.wait(deadline);
        Ok(())
    }

    pub fn disarm(&self) {
        self.set_masked(true);
        self.set_compare(u64::MAX);
        self.disable();
    }
}

#[inline]
fn to_ticks(d: Duration, freq_hz: u64) -> Option<u64> {
    if freq_hz == 0 {
        return None;
    }

    let secs = d.as_secs();
    let ns = d.subsec_nanos() as u64;

    let a = secs.checked_mul(freq_hz)?;
    let b = ns.checked_mul(freq_hz)?.checked_div(1_000_000_000)?;
    let total = a.checked_add(b)?;

    Some(total)
}

#[inline]
fn read_cntfrq_el0() -> u64 {
    CNTFRQ_EL0.get()
}

#[inline]
fn read_cntvct_el0() -> u64 {
    CNTVCT_EL0.get()
}

#[inline]
fn read_cntpct_el0() -> u64 {
    CNTPCT_EL0.get()
}

#[inline]
fn read_cntv_cval_el0() -> u64 {
    CNTV_CVAL_EL0.get()
}

#[inline]
fn read_cntp_cval_el0() -> u64 {
    CNTP_CVAL_EL0.get()
}

#[inline]
fn write_cntv_cval_el0(val: u64) {
    CNTV_CVAL_EL0.set(val);
}

#[inline]
fn write_cntp_cval_el0(val: u64) {
    CNTV_CVAL_EL0.set(val);
}
