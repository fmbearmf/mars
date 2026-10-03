use super::PAGE_DESCRIPTORS;
use crate::pm::page::mapper::AddressTranslator;
use crate::{
    pm::page::mapper::TableAllocator,
    vm::{VmError, page_allocator::PhysicalPageAllocator},
};
use hal::paging::PageTable;

// i hate this. hate hate hate hate hate.
// but i don't have a better solution, so it is what it is
pub struct UserAllocator<'a>(
    pub &'a dyn TableAllocator,
    pub &'a dyn PhysicalPageAllocator,
    pub &'a dyn AddressTranslator,
);
impl TableAllocator for UserAllocator<'_> {
    fn alloc_table(&self) -> Result<PageTable, VmError> {
        let t = self.0.alloc_table()?;
        let pa = self.2.dmap_to_phys(t.as_ptr());
        PAGE_DESCRIPTORS.get_page_descriptor(pa).lock.write().meta = None;
        Ok(t)
    }
    fn free_table(&self, t: PageTable) {
        let pa = self.2.dmap_to_phys(t.as_ptr());
        PAGE_DESCRIPTORS.get_page_descriptor(pa).lock.write().meta = None;
        self.0.free_table(t);
    }
}
impl PhysicalPageAllocator for UserAllocator<'_> {
    fn alloc_phys_page(&self) -> Result<usize, VmError> {
        self.1.alloc_phys_page()
    }
    fn free_phys_page(&self, pa: usize) {
        self.1.free_phys_page(pa)
    }
}
