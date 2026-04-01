use core::arch::asm;

use tock_registers::interfaces::{Readable, Writeable};
use tock_registers::register_bitfields;

register_bitfields! {u64,
    pub ICC_IGRPEN1_EL1 [
        /// Enable Group 1 interupts for the current Security state
        ENABLE      OFFSET(0) NUMBITS(1) [ Disabled = 0, Enabled = 1 ],
    ]
}

pub struct Reg;

impl Readable for Reg {
    type T = u64;
    type R = ICC_IGRPEN1_EL1::Register;

    fn get(&self) -> Self::T {
        let value: u64;
        unsafe { asm!("mrs {0}, icc_igrpen1_el1", out(reg) value) };
        value
    }
}

impl Writeable for Reg {
    type T = u64;
    type R = ICC_IGRPEN1_EL1::Register;

    fn set(&self, value: Self::T) {
        unsafe { asm!("msr icc_igrpen1_el1, {0}", in(reg) value) }
    }
}

pub const ICC_IGRPEN1_EL1: Reg = Reg {};
