use core::{arch::asm, hash::BuildHasherDefault};

use aarch64_cpu::registers::{MPIDR_EL1, Readable};
use hashbrown::HashMap;
use rustc_hash::FxHasher;

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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct CpuTopologyId(u32);

impl CpuTopologyId {
    pub const fn new(affinities: u32) -> Self {
        Self(affinities)
    }

    pub const fn from_mpidr(mpidr: u64) -> Self {
        let aff3 = ((mpidr >> 32) & 0xFF) as u32;
        let aff2 = ((mpidr >> 16) & 0xFF) as u32;
        let aff1 = ((mpidr >> 8) & 0xFF) as u32;
        let aff0 = (mpidr & 0xFF) as u32;
        Self(aff0 | (aff1 << 8) | (aff2 << 16) | (aff3 << 24))
    }

    pub fn current() -> Self {
        Self::from_mpidr(MPIDR_EL1.get())
    }

    pub const fn to_mpidr(&self) -> u64 {
        let (aff3, aff2, aff1, aff0) = mpidr_affinities(self.0);

        aff0 as u64 | ((aff1 as u64) << 8) | ((aff2 as u64) << 16) | ((aff3 as u64) << 32)
    }
}

type Map<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

static CPU_ID_MAP: Map<(u8, u8), u32> = Map::with_hasher(BuildHasherDefault::new());

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct CpuIdLogical(u32);

#[derive(Debug)]
pub struct SecondaryBootArgs {
    pub ttbr0: u64,
    pub ttbr1: u64,
    pub tcr: u64,
    pub mair: u64,
    pub stack_top_virt: u64,
    pub entry_virt: u64,
    pub sctlr: u64,
}

pub const fn mpidr_affinities(mpidr: u32) -> (u8, u8, u8, u8) {
    let aff0 = mpidr & 0xFF;
    let aff1 = (mpidr >> 8) & 0xFF;
    let aff2 = (mpidr >> 16) & 0xFF;
    let aff3 = (mpidr >> 24) & 0xFF;

    (aff3 as u8, aff2 as u8, aff1 as u8, aff0 as u8)
}
