pub use hal::paging::{Access, Entry, Level, MappingOptions, MemoryType, PageTable};
use hal::paging::{GEOMETRY, PagingError};

use crate::{
    allocator_support::KernelAddressTranslator,
    pm::page::PageAllocator,
    vm::{page_allocator::KernelPTAllocator, slab::SlabAllocator},
};

pub mod page_allocator;
pub mod slab;
pub mod user;

/// use `KALLOCATOR`
pub static KPAGE_ALLOCATOR: PageAllocator = PageAllocator::new(&KernelAddressTranslator);

pub static KPT_ALLOCATOR: KernelPTAllocator = KernelPTAllocator {};

pub static KALLOCATOR: SlabAllocator =
    SlabAllocator::new(&KPAGE_ALLOCATOR, &KernelAddressTranslator);

/// arbitrary policy
pub const DMAP_START: usize = 0xFFFF_0000_0000_0000;

pub const PAGE_SHIFT: usize = GEOMETRY.page_shift();
pub const PAGE_SIZE: usize = GEOMETRY.page_size();
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

/// map a borrowed device region through dmap.
/// identical existing mappings are retained and conflicting mappings are rejected.
///
/// safety:
/// the range must be live mmio, not ram.
/// usual mmio rules: device accesses must be serialized, and references should not be taken
pub unsafe fn map_mmio(physical: usize, size: usize) -> Result<*mut u8, VmError> {
    use crate::pm::page::mapper::AddressTranslator;

    let end = physical.checked_add(size).ok_or(VmError::InvalidAddress)?;
    if size == 0 {
        return Err(VmError::InvalidSize);
    }

    let first = align_down(physical, PAGE_SIZE);
    let last = end.checked_add(PAGE_MASK).ok_or(VmError::InvalidAddress)? & !PAGE_MASK;
    hal::paging::validate_mapping(first, Level::LEAF, MappingOptions::MMIO)?;
    hal::paging::validate_mapping(last - PAGE_SIZE, Level::LEAF, MappingOptions::MMIO)?;
    let virtual_start = KernelAddressTranslator.phys_to_dmap(first) as usize;
    let virtual_end = virtual_start
        .checked_add(last - first)
        .ok_or(VmError::InvalidAddress)?;
    let mut cursor =
        unsafe { user::address_space::KERNEL_ADDRESS_SPACE.lock(virtual_start..virtual_end) }?;
    unsafe { cursor.map(first, MappingOptions::MMIO) }?;

    Ok(KernelAddressTranslator.phys_to_dmap(physical))
}

pub const fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}

pub const fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VmError {
    Overlap,
    InvalidAddress,
    InvalidSize,
    OutOfMemory,
    InvalidAlignment,
    NotMapped,
    Unsupported,
    AlreadyInitialized,
    NotInitialized,
}

impl From<VmError> for PagingError {
    fn from(error: VmError) -> Self {
        match error {
            VmError::Overlap | VmError::AlreadyInitialized => Self::AlreadyMapped,
            VmError::InvalidAddress => Self::InvalidAddress,
            VmError::InvalidSize => Self::InvalidSize,
            VmError::OutOfMemory => Self::OutOfMemory,
            VmError::InvalidAlignment => Self::InvalidAlignment,
            VmError::NotMapped | VmError::NotInitialized => Self::NotMapped,
            VmError::Unsupported => Self::Unsupported,
        }
    }
}

impl From<PagingError> for VmError {
    fn from(error: PagingError) -> Self {
        match error {
            PagingError::InvalidAddress => Self::InvalidAddress,
            PagingError::InvalidAlignment => Self::InvalidAlignment,
            PagingError::InvalidSize => Self::InvalidSize,
            PagingError::Unsupported => Self::Unsupported,
            PagingError::OutOfMemory => Self::OutOfMemory,
            PagingError::AlreadyMapped => Self::Overlap,
            PagingError::NotMapped => Self::NotMapped,
        }
    }
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
    phys_addr | DMAP_START as u64
}

pub const fn dmap_addr_to_phys(dmap_addr: u64) -> u64 {
    dmap_addr & !DMAP_START as u64
}

#[inline]
pub const fn is_kernel_address(addr: usize) -> bool {
    addr >= DMAP_START
}
