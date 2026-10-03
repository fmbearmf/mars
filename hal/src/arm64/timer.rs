use aarch64_cpu::asm::barrier::{self, isb};
use aarch64_cpu::registers::{
    CNTFRQ_EL0, CNTP_CTL_EL0, CNTP_CVAL_EL0, CNTPCT_EL0, CNTV_CTL_EL0, CNTV_CVAL_EL0, CNTVCT_EL0,
    CurrentEL, ReadWriteable, Readable, Writeable,
};

fn physical() -> bool {
    CurrentEL.read(CurrentEL::EL) == 2
}

pub(crate) fn interrupt_from_acpi(table: &[u8]) -> Option<u32> {
    if table.get(..4)? != b"GTDT" {
        return None;
    }
    let length = u32::from_le_bytes(table.get(4..8)?.try_into().ok()?) as usize;
    if length > table.len() || length < 64 {
        return None;
    }
    let offset = if physical() { 60 } else { 52 };
    Some(u32::from_le_bytes(
        table.get(offset..offset + 4)?.try_into().ok()?,
    ))
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
