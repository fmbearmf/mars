use core::{arch::asm, fmt::Display, hash::BuildHasherDefault};

use aarch64_cpu::registers::{MPIDR_EL1, Readable, TPIDR_EL1};
use alloc::vec::Vec;
use atomic_refcell::AtomicRefCell;
use hashbrown::HashMap;
use log::trace;
use rustc_hash::FxHasher;

use crate::{pm::page::mapper::id_map, this_cpu};

use super::interrupt::InterruptInterface;

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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
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

    pub fn to_logical(self) -> Option<CpuIdLogical> {
        CPU_ID_MAP.borrow().get(&self).copied()
    }
}

type Map<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;

static CPU_ID_MAP: AtomicRefCell<Map<CpuTopologyId, CpuIdLogical>> =
    AtomicRefCell::new(Map::with_hasher(BuildHasherDefault::new()));
static CPU_TOPOLOGIES: AtomicRefCell<Vec<CpuTopologyId>> = AtomicRefCell::new(Vec::new());

pub fn init_cpu_maps(topologies: impl IntoIterator<Item = CpuTopologyId>) {
    trace!("init cpu maps");
    let mut id_map = CPU_ID_MAP.borrow_mut();
    let mut topology_map = CPU_TOPOLOGIES.borrow_mut();

    id_map.clear();
    topology_map.clear();

    for (i, topology) in topologies.into_iter().enumerate() {
        let logical = CpuIdLogical::new(i as u32);
        topology_map.push(topology);
        id_map.insert(topology, logical);
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct CpuIdLogical(u32);

impl Display for CpuIdLogical {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl CpuIdLogical {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    pub const fn to_usize(self) -> usize {
        self.0 as _
    }

    pub const fn to_u32(self) -> u32 {
        self.0 as _
    }

    /// current CPU's logical ID.
    pub fn current() -> Self {
        let tpidr = TPIDR_EL1.get();
        if tpidr == 0 {
            CpuTopologyId::current()
                .to_logical()
                .unwrap_or(Self::new(0))
        } else {
            this_cpu!().id
        }
    }
}

pub const fn mpidr_affinities(mpidr: u32) -> (u8, u8, u8, u8) {
    let aff0 = mpidr & 0xFF;
    let aff1 = (mpidr >> 8) & 0xFF;
    let aff2 = (mpidr >> 16) & 0xFF;
    let aff3 = (mpidr >> 24) & 0xFF;

    (aff3 as u8, aff2 as u8, aff1 as u8, aff0 as u8)
}
