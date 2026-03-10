use core::arch::asm;

use super::InterruptInterface;

pub struct Arm64InterruptInterface;

impl Arm64InterruptInterface {
    #[inline(always)]
    fn read_iar() -> u32 {
        let val: u32;
        unsafe {
            asm!("mrs {}, ICC_IAR1_EL1", out(reg) val);
        }
        val
    }

    #[inline(always)]
    fn write_eoir1(val: u32) {
        unsafe {
            asm!("msr ICC_EOIR1_EL1, {}", in(reg) val as u64);
        }
    }

    #[inline(always)]
    fn write_pmr(val: u8) {
        unsafe {
            asm!("msr ICC_PMR_EL1, {}", in(reg) val as u64);
        }
    }

    #[inline(always)]
    fn write_igrpen1(val: u64) {
        unsafe {
            asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) val);
        }
    }
}

impl InterruptInterface for Arm64InterruptInterface {
    fn read_iar(&self) -> u32 {
        Arm64InterruptInterface::read_iar()
    }

    fn write_eoir(&self, int_id: u32) {
        Arm64InterruptInterface::write_eoir1(int_id);
    }

    fn enable_group1(&self) {
        Arm64InterruptInterface::write_igrpen1(1);
    }

    fn disable_group1(&self) {
        Arm64InterruptInterface::write_igrpen1(0);
    }

    fn set_priority_mask(&self, mask: u8) {
        Arm64InterruptInterface::write_pmr(mask);
    }
}
