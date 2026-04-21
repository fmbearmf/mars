use core::ptr::NonNull;

use super::super::{TABLE_ENTRIES, TTable, page_allocator::PhysicalPageAllocator};
use super::PAGE_DESCRIPTORS;
use crate::pm::page::mapper::{AddressTranslator, TableAllocator};

/// thin wrapper that updates the `PageDescriptors` global
pub struct UserAllocator<'a, T, P, A>(pub &'a T, pub &'a P, pub core::marker::PhantomData<A>);

impl<'a, T: TableAllocator, P, A: AddressTranslator> TableAllocator for UserAllocator<'a, T, P, A> {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let ptr = self.0.alloc_table();
        let pa = A::dmap_to_phys(ptr.as_ptr());

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(pa as usize);
        let meta_ref = &mut desc.lock.write().meta;
        *meta_ref = None;

        ptr
    }

    fn free_table(&self, table: core::ptr::NonNull<TTable<TABLE_ENTRIES>>) {
        let pa = A::dmap_to_phys(table.as_ptr());

        let desc = PAGE_DESCRIPTORS.get_page_descriptor(pa as usize);
        desc.lock.write().meta = None;
        self.0.free_table(table);
    }
}

impl<'a, T, P: PhysicalPageAllocator, A> PhysicalPageAllocator for UserAllocator<'a, T, P, A> {
    fn alloc_phys_page<E: Into<usize> + From<usize>>(&self) -> Result<E, crate::vm::VmError> {
        self.1.alloc_phys_page()
    }

    fn free_phys_page<E: Into<usize> + From<usize>>(&self, pa: E) {
        self.1.free_phys_page(pa)
    }
}
