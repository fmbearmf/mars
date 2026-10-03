use core::{alloc::Layout, ptr::NonNull};

#[path = "paging_context.rs"]
pub(crate) mod context;
pub use context::{AddressSpaceContext, TableMemory};

/// a PT level, counted upwards from the smallest mapping.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Level(u8);

impl Level {
    pub const LEAF: Self = Self(0);
    pub const ROOT: Self = Self(GEOMETRY.levels as u8 - 1);

    pub const fn new(level: u8) -> Option<Self> {
        if (level as usize) < GEOMETRY.levels {
            Some(Self(level))
        } else {
            None
        }
    }
    pub const fn get(self) -> u8 {
        self.0
    }
    pub const fn child(self) -> Option<Self> {
        if self.0 == 0 {
            None
        } else {
            Some(Self(self.0 - 1))
        }
    }
    pub const fn parent(self) -> Option<Self> {
        Self::new(self.0 + 1)
    }
    pub const fn coverage(self) -> usize {
        GEOMETRY.mapping_size(self)
    }
    pub const fn index(self, address: usize) -> usize {
        (address >> GEOMETRY.shift(self)) & (GEOMETRY.entries_at(self) - 1)
    }
}

/// paging geometry. aims to support every modern architecture that uses radix tree page tables (x86, ARM, RISC-V, etc.)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Geometry {
    page_shift: usize,
    index_bits: usize,
    levels: usize,
    virtual_bits: usize,
    table_bytes: usize,
    table_alignment: usize,
}

impl Geometry {
    pub(crate) const fn new(
        page_shift: usize,
        index_bits: usize,
        levels: usize,
        virtual_bits: usize,
        table_bytes: usize,
        table_alignment: usize,
    ) -> Self {
        Self {
            page_shift,
            index_bits,
            levels,
            virtual_bits,
            table_bytes,
            table_alignment,
        }
    }
    pub const fn page_size(self) -> usize {
        1 << self.page_shift
    }
    pub const fn page_shift(self) -> usize {
        self.page_shift
    }
    pub const fn entries(self) -> usize {
        1 << self.index_bits
    }
    pub const fn entries_at(self, level: Level) -> usize {
        let remaining = self.virtual_bits - self.shift(level);
        1 << if remaining < self.index_bits {
            remaining
        } else {
            self.index_bits
        }
    }
    pub const fn levels(self) -> usize {
        self.levels
    }
    pub const fn virtual_address_bits(self) -> usize {
        self.virtual_bits
    }
    pub fn table_layout(self) -> Layout {
        Layout::from_size_align(self.table_bytes, self.table_alignment)
            .expect("invalid HAL table layout")
    }
    pub const fn mapping_size(self, level: Level) -> usize {
        1 << self.shift(level)
    }
    const fn shift(self, level: Level) -> usize {
        self.page_shift + self.index_bits * level.0 as usize
    }
}

pub const GEOMETRY: Geometry = crate::arch::GEOMETRY;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Access {
    KernelReadOnly,
    KernelReadWrite,
    UserReadOnly,
    UserReadWrite,
}
impl Access {
    pub const fn writable(self) -> bool {
        matches!(self, Self::KernelReadWrite | Self::UserReadWrite)
    }
    pub const fn user(self) -> bool {
        matches!(self, Self::UserReadOnly | Self::UserReadWrite)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MemoryType {
    Normal,
    WriteThrough,
    Uncached,
    Device,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MappingOptions {
    pub access: Access,
    pub executable: bool,
    pub memory_type: MemoryType,
}
impl MappingOptions {
    pub const KERNEL_RW: Self = Self {
        access: Access::KernelReadWrite,
        executable: false,
        memory_type: MemoryType::Normal,
    };
    pub const KERNEL_CODE: Self = Self {
        access: Access::KernelReadOnly,
        executable: true,
        memory_type: MemoryType::Normal,
    };
    pub const MMIO: Self = Self {
        access: Access::KernelReadWrite,
        executable: false,
        memory_type: MemoryType::Device,
    };
}

/// atomic snapshot
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Invalid,
    Table {
        physical_address: usize,
    },
    Mapping {
        physical_address: usize,
        options: MappingOptions,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PagingError {
    InvalidAddress,
    InvalidAlignment,
    InvalidSize,
    Unsupported,
    OutOfMemory,
    AlreadyMapped,
    NotMapped,
}

/// Non-owning handle to table storage. Lifetime and software exclusion belong to its owner.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PageTable(NonNull<u8>);
impl PageTable {
    /// safety:
    /// storage must be live and at least `GEOMETRY.table_layout()` bytes
    pub unsafe fn from_ptr(pointer: *mut u8) -> Self {
        assert_eq!(
            pointer as usize & (GEOMETRY.table_layout().align() - 1),
            0,
            "unaligned table"
        );
        Self(NonNull::new(pointer).expect("null page table"))
    }
    pub fn as_ptr(self) -> *mut u8 {
        self.0.as_ptr()
    }
    /// safety:
    /// storage must be unpublished and exclusively owned
    pub unsafe fn zero(self) {
        unsafe { crate::arch::zero_table(self) }
    }

    /// safety:
    /// storage is and remains live. `level` must describe this table
    pub unsafe fn read(self, index: usize, level: Level) -> Entry {
        assert!(index < GEOMETRY.entries_at(level));
        unsafe { crate::arch::read_entry(self, index, level) }
    }

    /// replace a descriptor, including necessary invalidation.
    /// detached descendants may be reclaimed once (software) readers have also been excluded.
    ///
    /// safety:
    /// the storage is live, `level` is correct, and the caller exclusively owns the affected entry/subtree.
    /// new child storage must be completely initialized before publication
    pub unsafe fn write(self, index: usize, level: Level, entry: Entry) -> Result<(), PagingError> {
        if index >= GEOMETRY.entries_at(level) {
            return Err(PagingError::InvalidAddress);
        }
        unsafe { crate::arch::write_entry(self, index, level, entry) }
    }
}

pub fn valid_virtual_address(address: usize) -> bool {
    crate::arch::valid_virtual_address(address)
}

pub fn supports_mapping(level: Level) -> bool {
    crate::arch::supports_mapping(level)
}

/// Validates a descriptor without mutating table storage.
pub fn validate_mapping(
    physical: usize,
    level: Level,
    options: MappingOptions,
) -> Result<(), PagingError> {
    crate::arch::validate_mapping(physical, level, options)
}
