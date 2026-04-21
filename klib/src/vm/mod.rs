use core::{fmt::Debug, mem::transmute};

use aarch64_cpu_ext::structures::tte::{TTE4K48, TTE16K48};

pub mod page_allocator;
pub mod slab;
pub mod user;

pub type TTENATIVE = TTE16K48;
pub type TTEUEFI = TTE4K48;

pub const DMAP_START: usize = 0xFFFF << 48;

pub const PAGE_INDEX_BITS: usize = TABLE_ENTRIES.trailing_zeros() as usize;
pub const PAGE_SHIFT: usize = 14; // 16kib
pub const L2_BLOCK_SHIFT: usize = PAGE_SHIFT + PAGE_INDEX_BITS; // 32mib
pub const L1_BLOCK_SHIFT: usize = L2_BLOCK_SHIFT + PAGE_INDEX_BITS; // 64gib
pub const L0_BLOCK_SHIFT: usize = L1_BLOCK_SHIFT + PAGE_INDEX_BITS; // 128tib

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

impl<const N: usize> Debug for TTable<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entries: &[u64; N] = unsafe { transmute(&self.entries) };
        f.debug_struct("TTable").field("entries", entries).finish()
    }
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

const INVALID_ENTRY: TTENATIVE = TTENATIVE::invalid();

impl<const N: usize> TTable<N> {
    pub const fn new() -> Self {
        Self {
            entries: [INVALID_ENTRY; N],
        }
    }
}

#[derive(Debug)]
pub enum VmError {
    Overlap,
    InvalidAddress,
    InvalidSize,
    OutOfMemory,
    InvalidAlignment,
    NotMapped,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MemoryRegionType {
    Corrupt = 0,
    KernelCode,
    KernelRwData,
    KernelRoData,
    KernelStack,
    Mmio,
    BootloaderReclaim,
    FirmwareReclaim,

    AcpiTables,
    AcpiNvs,

    PageTable,

    RtFirmwareCode,
    RtFirmwareData,

    Normal,

    Unknown = 255,
}

impl MemoryRegionType {
    #[inline]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::KernelCode,
            2 => Self::KernelRwData,
            3 => Self::KernelRoData,
            4 => Self::KernelStack,
            5 => Self::Mmio,
            6 => Self::BootloaderReclaim,
            7 => Self::FirmwareReclaim,
            8 => Self::AcpiTables,
            9 => Self::AcpiNvs,
            10 => Self::PageTable,
            11 => Self::RtFirmwareCode,
            12 => Self::RtFirmwareData,
            13 => Self::Normal,
            255 => Self::Unknown,
            _ => Self::Corrupt,
        }
    }

    #[inline]
    pub const fn as_bits(self) -> u8 {
        self as u8
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: usize,
    pub size: usize,
    pub region_type: MemoryRegionType,
}

impl MemoryRegion {
    pub fn end(&self) -> usize {
        self.base + self.size
    }

    pub fn can_merge(&self, other: &MemoryRegion) -> bool {
        self.region_type == other.region_type &&
        // check if overlap or touch
        !(self.end() < other.base || other.end() < self.base)
    }

    pub fn merge(&mut self, other: MemoryRegion) {
        let start = self.base.min(other.base);
        let end = self.end().max(other.end());
        self.base = start;
        self.size = end - start;
    }

    pub fn is_normal(&self) -> bool {
        self.region_type == MemoryRegionType::Normal
    }

    pub fn is_usable(&self) -> bool {
        match self.region_type {
            MemoryRegionType::Normal
            | MemoryRegionType::BootloaderReclaim
            | MemoryRegionType::FirmwareReclaim => true,
            _ => false,
        }
    }
}

pub const fn phys_addr_to_dmap(phys_addr: u64) -> u64 {
    if is_kernel_address(phys_addr as usize) {
        return phys_addr;
    }
    DMAP_START as u64 + phys_addr
}

pub const fn dmap_addr_to_phys(dmap_addr: u64) -> u64 {
    dmap_addr
        .checked_sub(DMAP_START as u64)
        .unwrap_or(dmap_addr)
}

#[inline]
pub const fn bsize_for_level(level: usize) -> usize {
    let exp = 3usize.saturating_sub(level);
    let fac = TABLE_ENTRIES.pow(exp as u32);
    PAGE_SIZE * fac
}

#[inline]
pub const fn is_kernel_address(addr: usize) -> bool {
    ((addr >> 48) & 0x1) == 0x1
}
