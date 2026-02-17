use aarch64_cpu_ext::structures::tte::{TTE4K48, TTE16K48};
use spin::Mutex;

pub mod page;
pub mod slab;

pub type TTENATIVE = TTE16K48;
pub type TTEUEFI = TTE4K48;

pub const DMAP_START: usize = 0xFFFF << 48;

pub const PAGE_SHIFT: usize = 14; // 16kib
pub const L2_BLOCK_SHIFT: usize = PAGE_SHIFT + 11; // 32mib
pub const L1_BLOCK_SHIFT: usize = L2_BLOCK_SHIFT + 11; // 64gib
pub const L0_BLOCK_SHIFT: usize = L1_BLOCK_SHIFT + 11; // 128tib

pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
pub const L2_BLOCK_SIZE: usize = 1 << L2_BLOCK_SHIFT;
pub const L1_BLOCK_SIZE: usize = 1 << L1_BLOCK_SHIFT;
pub const L0_BLOCK_SIZE: usize = 1 << L0_BLOCK_SHIFT;

pub const PAGE_MASK: usize = PAGE_SIZE - 1;
pub const L2_BLOCK_MASK: usize = L2_BLOCK_SIZE - 1;
pub const L1_BLOCK_MASK: usize = L1_BLOCK_SIZE - 1;
pub const L0_BLOCK_MASK: usize = L0_BLOCK_SIZE - 1;

pub const TABLE_ENTRIES: usize =
    aarch64_cpu_ext::structures::tte::block_sizes::granule_16k::LEVEL3_PAGE_SIZE / 8usize;

pub const MAIR_NORMAL_INDEX: u64 = 1;
pub const MAIR_DEVICE_INDEX: u64 = 0;

#[derive(Copy, Clone)]
#[repr(C, align(16384))]
pub struct TTable<const N: usize> {
    pub entries: [TTENATIVE; N],
}

#[derive(Copy, Clone)]
#[repr(C, align(4096))]
pub struct TTableUEFI {
    pub entries: [TTEUEFI; 512],
}

pub const fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}

pub const fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

impl<const N: usize> TTable<N> {
    pub const fn new() -> Self {
        Self {
            entries: [TTENATIVE::invalid(); N],
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: usize,
    pub size: usize,
}

pub const fn phys_addr_to_dmap(phys_addr: u64) -> u64 {
    DMAP_START as u64 + phys_addr
}

#[inline]
pub const fn bsize_for_level(level: usize) -> usize {
    let exp = 3usize.saturating_sub(level);
    let fac = TABLE_ENTRIES.pow(exp as u32);
    PAGE_SIZE * fac
}
