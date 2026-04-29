use core::ptr::NonNull;

use super::super::{TABLE_ENTRIES, TTable, page_allocator::PhysicalPageAllocator};
use super::PAGE_DESCRIPTORS;
use crate::pm::page::mapper::{AddressTranslator, TableAllocator};

/// thin wrapper that updates the `PageDescriptors` global
pub struct UserAllocator<'a>(
    pub &'a dyn TableAllocator,
    pub &'a dyn PhysicalPageAllocator,
    pub &'a dyn AddressTranslator,
);

impl TableAllocator for UserAllocator<'_> {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let ptr = self.0.alloc_table();
        let pa = self.2.dmap_to_phys(ptr.as_ptr() as _);

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(pa as usize);
        let meta_ref = &mut desc.lock.write().meta;
        *meta_ref = None;

        ptr
    }

    fn free_table(&self, table: core::ptr::NonNull<TTable<TABLE_ENTRIES>>) {
        let pa = self.2.dmap_to_phys(table.as_ptr() as _);

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(pa as usize);
        desc.lock.write().meta = None;
        self.0.free_table(table);
    }
}

impl PhysicalPageAllocator for UserAllocator<'_> {
    fn alloc_phys_page(&self) -> Result<usize, crate::vm::VmError> {
        self.1.alloc_phys_page()
    }

    fn free_phys_page(&self, pa: usize) {
        self.1.free_phys_page(pa)
    }
}
