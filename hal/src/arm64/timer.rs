use aarch64_cpu::asm::barrier::{self, isb};
use aarch64_cpu::registers::{
    CNTFRQ_EL0, CNTP_CTL_EL0, CNTP_CVAL_EL0, CNTPCT_EL0, CNTV_CTL_EL0, CNTV_CVAL_EL0, CNTVCT_EL0,
    CurrentEL, ReadWriteable, Readable, Writeable,
};

use crate::timer::TimerCapability;
use core::sync::atomic::{Ordering, compiler_fence};

pub(crate) fn timer_filter() -> impl Fn(&TimerCapability) -> bool {
    let expected_hypervisor = hypervisor();
    let expected_secure = false;
    let expected_virtual = !physical();

    move |cap: &TimerCapability| {
        cap.is_secure == expected_secure
            && cap.is_hypervisor == expected_hypervisor
            && cap.is_virtual == expected_virtual
    }
}

fn hypervisor() -> bool {
    CurrentEL.read(CurrentEL::EL) == 2
}

fn physical() -> bool {
    // false

    // virtual timer isn't guaranteed at EL2 because... ACPI
    hypervisor()
}

pub(crate) fn timer_frequency() -> u64 {
    CNTFRQ_EL0.get()
}

pub(crate) fn timer_counter() -> u64 {
    if physical() {
        CNTPCT_EL0.get()
    } else {
        CNTVCT_EL0.get()
    }
}

pub(crate) fn ordered_counter() -> u64 {
    compiler_fence(Ordering::SeqCst);

    unsafe { core::arch::asm!("isb", options(nomem, nostack, preserves_flags)) };
    let counter = timer_counter();
    unsafe { core::arch::asm!("isb", options(nomem, nostack, preserves_flags)) };

    compiler_fence(Ordering::SeqCst);

    counter
}

pub(crate) fn timer_deadline() -> u64 {
    if physical() {
        CNTP_CVAL_EL0.get()
    } else {
        CNTV_CVAL_EL0.get()
    }
}

pub(crate) fn timer_enabled() -> bool {
    if physical() {
        CNTP_CTL_EL0.matches_all(CNTP_CTL_EL0::ENABLE::SET)
    } else {
        CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::ENABLE::SET)
    }
}

pub(crate) fn timer_masked() -> bool {
    if physical() {
        CNTP_CTL_EL0.matches_all(CNTP_CTL_EL0::IMASK::SET)
    } else {
        CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::IMASK::SET)
    }
}

pub(crate) fn timer_pending() -> bool {
    if physical() {
        CNTP_CTL_EL0.matches_all(CNTP_CTL_EL0::ISTATUS::SET)
    } else {
        CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::ISTATUS::SET)
    }
}

pub(crate) fn timer_set_deadline(value: u64) {
    if physical() {
        CNTP_CVAL_EL0.set(value)
    } else {
        CNTV_CVAL_EL0.set(value)
    };

    isb(barrier::SY);
}

pub(crate) fn timer_enable() {
    if physical() {
        CNTP_CTL_EL0.modify(CNTP_CTL_EL0::ENABLE::SET)
    } else {
        CNTV_CTL_EL0.modify(CNTV_CTL_EL0::ENABLE::SET)
    };

    isb(barrier::SY);
}

pub(crate) fn timer_disable() {
    if physical() {
        CNTP_CTL_EL0.modify(CNTP_CTL_EL0::ENABLE::CLEAR)
    } else {
        CNTV_CTL_EL0.modify(CNTV_CTL_EL0::ENABLE::CLEAR)
    };

    isb(barrier::SY);
}

pub(crate) fn timer_set_masked(value: bool) {
    if physical() {
        CNTP_CTL_EL0.modify(if value {
            CNTP_CTL_EL0::IMASK::SET
        } else {
            CNTP_CTL_EL0::IMASK::CLEAR
        })
    } else {
        CNTV_CTL_EL0.modify(if value {
            CNTV_CTL_EL0::IMASK::SET
        } else {
            CNTV_CTL_EL0::IMASK::CLEAR
        })
    };

    isb(barrier::SY);
}
