pub mod address_space;
pub mod allocator;
pub mod cursor;

pub use address_space::AddressSpace;

use crate::{
    sync::RwLock,
    vm::{PAGE_MASK, PAGE_SHIFT, VmError},
};
use alloc::{boxed::Box, vec::Vec};
use core::ops::Range;
use hal::paging::{GEOMETRY, Level, MappingOptions};

pub struct PageDescriptors(RwLock<Option<(&'static [PageDescriptor], Range<usize>)>>);

impl PageDescriptors {
    pub const fn new() -> Self {
        Self(RwLock::new(None))
    }
    pub fn init(&self, descriptors: Box<[PageDescriptor]>, range: Range<usize>) {
        assert!(range.start < range.end && (range.start | range.end) & PAGE_MASK == 0);
        assert_eq!(descriptors.len(), (range.end - range.start) >> PAGE_SHIFT);
        let mut guard = self.0.write();
        assert!(guard.is_none(), "page descriptors already initialized");
        *guard = Some((Box::leak(descriptors), range));
    }
    pub fn get_page_descriptor(&self, physical: usize) -> &'static PageDescriptor {
        let guard = self.0.read();
        let (descriptors, range) = guard.as_ref().expect("page descriptors not initialized");
        assert_eq!(physical & PAGE_MASK, 0, "unaligned table address");
        assert!(
            range.contains(&physical),
            "physical address outside descriptor range"
        );
        &descriptors[(physical - range.start) >> PAGE_SHIFT]
    }
}
impl Default for PageDescriptors {
    fn default() -> Self {
        Self::new()
    }
}

pub static PAGE_DESCRIPTORS: PageDescriptors = PageDescriptors::new();

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Invalid,
    Mapped {
        pa: usize,
        options: MappingOptions,
    },
    PrivateAnonymous(MappingOptions),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StatusCategory {
    Allocated,
    Mapped,
}

impl Status {
    pub fn category(self) -> Option<StatusCategory> {
        match self {
            Self::Mapped { .. } => Some(StatusCategory::Mapped),
            Self::PrivateAnonymous(_) => Some(StatusCategory::Allocated),
            Self::Invalid => None,
        }
    }
}

/// software state of a PTE. resident mapping properties are read from HAL-provided snapshots
#[derive(Debug, Copy, Clone, Default)]
pub struct PteMeta {
    pub status: Status,
}

#[derive(Default)]
pub struct PtState {
    pub meta: Option<Box<[PteMeta]>>,
}

pub struct PageDescriptor {
    pub lock: RwLock<PtState>,
}

impl PageDescriptor {
    pub fn new() -> Self {
        Self {
            lock: RwLock::new(PtState::default()),
        }
    }
}

impl Default for PageDescriptor {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn new_meta(level: Level, status: Status) -> Result<Box<[PteMeta]>, VmError> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(GEOMETRY.entries_at(level))
        .map_err(|_| VmError::OutOfMemory)?;
    entries.resize(GEOMETRY.entries_at(level), PteMeta { status });
    Ok(entries.into_boxed_slice())
}
