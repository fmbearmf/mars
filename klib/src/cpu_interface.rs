use core::arch::asm;

use aarch64_cpu::registers::{MPIDR_EL1, Readable};

use super::{interrupt::InterruptInterface, vcpu::CpuDescriptor};

#[derive(Debug, Copy, Clone)]
pub struct Arm64InterruptInterface;

impl Arm64InterruptInterface {
    #[inline(always)]
    pub fn read_iar0() -> u32 {
        let val: u32;
        unsafe {
            asm!("mrs {0:x}, ICC_IAR0_EL1", out(reg) val);
        }
        val
    }

    #[inline(always)]
    pub fn read_iar1() -> u32 {
        let val: u32;
        unsafe {
            asm!("mrs {0:x}, ICC_IAR1_EL1", out(reg) val);
        }
        val
    }

    #[inline(always)]
    pub fn write_eoir0(val: u32) {
        unsafe {
            asm!("msr ICC_EOIR0_EL1, {}", in(reg) val as u64);
        }
    }

    #[inline(always)]
    pub fn write_eoir1(val: u32) {
        unsafe {
            asm!("msr ICC_EOIR1_EL1, {}", in(reg) val as u64);
        }
    }

    #[inline(always)]
    pub fn write_dir(val: u32) {
        unsafe {
            asm!("msr ICC_DIR_EL1, {}", in(reg) val as u64);
        }
    }

    #[inline(always)]
    pub fn write_pmr(val: u8) {
        unsafe {
            asm!("msr ICC_PMR_EL1, {}", in(reg) val as u64);
        }
    }

    #[inline(always)]
    pub fn write_igrpen0(val: u64) {
        unsafe {
            asm!("msr ICC_IGRPEN0_EL1, {}", in(reg) val);
        }
    }

    #[inline(always)]
    pub fn write_igrpen1(val: u64) {
        unsafe {
            asm!("msr ICC_IGRPEN1_EL1, {}", in(reg) val);
        }
    }
}

impl InterruptInterface for Arm64InterruptInterface {
    fn read_iar(&self) -> u32 {
        Arm64InterruptInterface::read_iar1()
    }

    fn write_eoir(&self, int_id: u32) {
        Arm64InterruptInterface::write_eoir1(int_id);
        Arm64InterruptInterface::write_dir(int_id);
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

#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct Mpidr(u64);

impl Mpidr {
    pub const fn new(aff3: u8, aff2: u8, aff1: u8, aff0: u8) -> Self {
        Self(((aff3 as u64) << 32) | ((aff2 as u64) << 16) | ((aff1 as u64) << 8) | (aff0 as u64))
    }

    #[inline]
    pub fn current() -> Self {
        Self(MPIDR_EL1.get())
    }

    pub fn affinity_only(&self) -> u64 {
        mpidr_key(self.0)
    }
}

#[derive(Debug)]
pub struct SecondaryBootArgs {
    pub ttbr0: u64,
    pub ttbr1: u64,
    pub tcr: u64,
    pub mair: u64,
    pub stack_top_virt: u64,
    pub entry_virt: u64,
    pub sctlr: u64,
    pub cpu_desc: *const CpuDescriptor,
}

pub fn mpidr_key(mpidr: u64) -> u64 {
    let aff0 = MPIDR_EL1::Aff0.read(mpidr);
    let aff1 = MPIDR_EL1::Aff1.read(mpidr);
    let aff2 = MPIDR_EL1::Aff2.read(mpidr);
    let aff3 = MPIDR_EL1::Aff3.read(mpidr);

    (aff3 << 32) | (aff2 << 16) | (aff1 << 8) | aff0
}

pub fn mpidr_affinities(mpidr: u64) -> (u8, u8, u8, u8) {
    let aff0 = MPIDR_EL1::Aff0.read(mpidr) as u8;
    let aff1 = MPIDR_EL1::Aff1.read(mpidr) as u8;
    let aff2 = MPIDR_EL1::Aff2.read(mpidr) as u8;
    let aff3 = MPIDR_EL1::Aff3.read(mpidr) as u8;

    (aff3, aff2, aff1, aff0)
}
