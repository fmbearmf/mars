use aarch64_cpu::registers::TTBR0_EL1;
use aarch64_cpu_ext::structures::tte::TTE16K48;
use spin::Mutex;

pub mod page;
pub mod slab;

pub const PAGE_SHIFT: usize = 14; // 16kib
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

pub type TTENATIVE = TTE16K48;

pub const TABLE_ENTRIES: usize =
    aarch64_cpu_ext::structures::tte::block_sizes::granule_16k::LEVEL3_PAGE_SIZE / 8usize;

#[derive(Copy, Clone)]
#[repr(C, align(16384))]
pub struct TTable<const N: usize> {
    pub entries: [TTENATIVE; N],
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

// L0
pub static ROOT_TTBR1_TABLE: Mutex<TTable<2>> = Mutex::new(TTable {
    entries: [TTENATIVE::invalid(); 2],
});

pub const fn mmio_addr_to_iomap(mmio_addr: u64) -> u64 {
    0xffff_8010_0000_0000 + mmio_addr
}
