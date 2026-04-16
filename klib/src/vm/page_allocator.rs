use super::VmError;

pub trait PhysicalPageAllocator {
    fn alloc_phys_page<T: Into<usize> + From<usize>>(&self) -> Result<T, VmError>;
    fn free_phys_page<T: Into<usize> + From<usize>>(&self, pa: T);
}

pub trait DmapPageAllocator {
    fn alloc_dmap_page<T: Into<usize> + From<usize>>(&self) -> Result<T, VmError>;
    fn free_dmap_page<T: Into<usize> + From<usize>>(&self, pa: T);
}
