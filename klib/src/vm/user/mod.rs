//! pages as they exist within virtual address spaces,
//! as opposed to backing pages (within RAM)

pub mod address_space;
pub mod allocator;
pub mod cursor;

extern crate alloc;

use core::{ops::Range, usize};

use aarch64_cpu_ext::structures::tte::AccessPermission;
use alloc::boxed::Box;

use super::{PAGE_SHIFT, TABLE_ENTRIES};
use crate::sync::RwLock;

#[repr(transparent)]
pub struct PageDescriptors(RwLock<Option<(&'static [PageDescriptor], Range<usize>)>>);

impl PageDescriptors {
    pub const fn new() -> Self {
        Self(RwLock::new(None))
    }

    pub fn init(&self, descriptors: Box<[PageDescriptor]>, range: Range<usize>) {
        let mut guard = self.0.write();
        assert!(guard.is_none(), "double init on `PageDescriptors`");

        // memory will live for the lifetime of the kernel. ie "leaking" isn't an issue
        *guard = Some((Box::leak(descriptors), range));
    }

    pub fn get_page_descriptor(&self, pa: usize) -> &PageDescriptor {
        let guard = self.0.read();

        let descs = guard.as_ref().expect("`PageDescriptors` uninitialized");

        assert!(
            descs.1.contains(&pa),
            "no page descriptor exists for requested address"
        );

        let pa = pa - descs.1.start;

        let pfn = pa >> PAGE_SHIFT;
        &descs.0[pfn]
    }
}

pub static PAGE_DESCRIPTORS: PageDescriptors = PageDescriptors::new();

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Status {
    /// Invalid entry.
    Invalid,

    /// Currently mapped to some PA with some permissions.
    Mapped { pa: usize, perm: AccessPermission },

    /// Allocated but unbacked memory.
    PrivateAnonymous(AccessPermission),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum StatusCategory {
    /// Page has been virtually allocated but backing memory hasn't been created.
    Reserved,
    /// Page is currently mapped to backing memory.
    Mapped,
    /// Page was mapped, but has been unmapped (swap, etc.)
    TemporarilyUnmapped,
}

impl Status {
    pub fn category(&self) -> Option<StatusCategory> {
        match self {
            Self::Mapped { .. } => Some(StatusCategory::Mapped),
            Self::PrivateAnonymous(..) => Some(StatusCategory::Reserved),
            Self::Invalid => None,
        }
    }
}

impl Default for Status {
    fn default() -> Self {
        Self::Invalid
    }
}

/// adapted from CortenMM (https://zhou-diyu.github.io/files/cortenmm-sosp25.pdf)
#[derive(Debug, Copy, Clone, Default)]
#[repr(transparent)]
pub struct PteMeta {
    pub status: Status,
}

type PteMetaArray = [PteMeta; TABLE_ENTRIES];

/// state protected by a page table page's lock
/// adapted from CortenMM (https://zhou-diyu.github.io/files/cortenmm-sosp25.pdf)
#[repr(transparent)]
pub struct PtState {
    pub meta: Option<Box<PteMetaArray>>,
}

#[repr(transparent)]
pub struct PageDescriptor {
    pub lock: RwLock<PtState>,
}

#[inline]
pub fn entry_index(addr: usize, level: usize) -> usize {
    (addr >> (PAGE_SHIFT + level * TABLE_ENTRIES.trailing_zeros() as usize)) & (TABLE_ENTRIES - 1)
}

#[inline]
pub fn entry_cover(level: usize) -> usize {
    1 << (PAGE_SHIFT + level * TABLE_ENTRIES.trailing_zeros() as usize)
}
