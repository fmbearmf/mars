use super::VmError;

pub trait PhysicalPageAllocator {
    fn alloc_phys_page(&self) -> Result<usize, VmError>;
    fn free_phys_page(&self, pa: usize);
}

pub trait DmapPageAllocator {
    fn alloc_dmap_page(&self) -> Result<usize, VmError>;
    fn free_dmap_page(&self, pa: usize);
}
