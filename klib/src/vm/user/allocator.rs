use super::super::{TABLE_ENTRIES, TTable, page_allocator::PhysicalPageAllocator};
use super::PAGE_DESCRIPTORS;
use crate::pm::page::mapper::TableAllocator;

/// thin wrapper that updates the `PageDescriptors` global
pub struct UserAllocator<'a, A, P>(pub &'a A, pub &'a P);

impl<'a, A: TableAllocator, P> TableAllocator for UserAllocator<'a, A, P> {
    fn alloc_table(&self) -> core::ptr::NonNull<TTable<TABLE_ENTRIES>> {
        let ptr = self.0.alloc_table();
        let pa = self.0.virt_to_phys(ptr.as_ptr());

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(pa as usize);
        desc.lock.write().meta = None;
        ptr
    }

    fn free_table(&self, table: core::ptr::NonNull<TTable<TABLE_ENTRIES>>) {
        let pa = self.0.virt_to_phys(table.as_ptr());

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(pa as usize);
        desc.lock.write().meta = None;
        self.0.free_table(table);
    }

    fn phys_to_virt(&self, phys: u64) -> *mut TTable<TABLE_ENTRIES> {
        self.0.phys_to_virt(phys)
    }

    fn virt_to_phys(&self, virt: *mut TTable<TABLE_ENTRIES>) -> u64 {
        self.0.virt_to_phys(virt)
    }
}

impl<'a, A, P: PhysicalPageAllocator> PhysicalPageAllocator for UserAllocator<'a, A, P> {
    fn alloc_phys_page(&self) -> Result<usize, crate::vm::VmError> {
        self.1.alloc_phys_page()
    }

    fn free_phys_page(&self, pa: usize) {
        self.1.free_phys_page(pa)
    }
}
