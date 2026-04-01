use core::arch::asm;

use tock_registers::interfaces::{Readable, Writeable};
use tock_registers::register_bitfields;

register_bitfields! {u64,
    pub ICC_PMR_EL1 [
        /// Priority Mask
        PMR         OFFSET(0) NUMBITS(8) [],
    ]
}

pub struct Reg;

impl Readable for Reg {
    type T = u64;
    type R = ICC_PMR_EL1::Register;

    fn get(&self) -> Self::T {
        let value: u64;
        unsafe { asm!("mrs {0}, icc_pmr_el1", out(reg) value) };
        value
    }
}

impl Writeable for Reg {
    type T = u64;
    type R = ICC_PMR_EL1::Register;

    fn set(&self, value: Self::T) {
        unsafe { asm!("msr icc_pmr_el1, {0}", in(reg) value) }
    }
}

pub const ICC_PMR_EL1: Reg = Reg {};
