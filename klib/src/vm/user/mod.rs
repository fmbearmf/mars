//! pages as they exist within virtual address spaces,
//! as opposed to backing pages (within RAM)

pub mod address_space;
pub mod allocator;
pub mod cursor;

extern crate alloc;

use aarch64_cpu_ext::structures::tte::AccessPermission;
use alloc::boxed::Box;

use super::{PAGE_SHIFT, TABLE_ENTRIES};
use crate::sync::RwLock;

#[repr(transparent)]
pub struct PageDescriptors(RwLock<Option<&'static [PageDescriptor]>>);

impl PageDescriptors {
    pub const fn new() -> Self {
        Self(RwLock::new(None))
    }

    pub fn init(&self, descriptors: Box<[PageDescriptor]>) {
        let mut guard = self.0.write();
        assert!(guard.is_none(), "double init on `PageDescriptors`");

        // memory will live for the lifetime of the kernel. ie "leaking" isn't an issue
        *guard = Some(Box::leak(descriptors));
    }

    pub fn get_page_descriptor(&self, pa: usize) -> &PageDescriptor {
        let guard = self.0.read();

        let descs = guard.expect("`PageDescriptors` uninitialized");

        let pfn = pa >> PAGE_SHIFT;
        &descs[pfn]
    }
}

pub static PAGE_DESCRIPTORS: PageDescriptors = PageDescriptors::new();

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Status {
    Invalid,
    Mapped { pa: usize, perm: AccessPermission },
    PrivateAnonymous(AccessPermission),
}

impl Default for Status {
    fn default() -> Self {
        Self::Invalid
    }
}

/// adapted from CortenMM (https://zhou-diyu.github.io/files/cortenmm-sosp25.pdf)
#[derive(Debug, Copy, Clone, Default)]
pub struct PteMeta {
    pub status: Status,
    pub shared: bool,
    pub writable: bool,
}

/// state protected by a page table page's lock
/// adapted from CortenMM (https://zhou-diyu.github.io/files/cortenmm-sosp25.pdf)
pub struct PtState {
    pub meta: Option<Box<[PteMeta; TABLE_ENTRIES]>>,
}

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
