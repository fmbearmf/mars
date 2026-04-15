use super::VmError;

pub trait PhysicalPageAllocator {
    fn alloc_phys_page(&self) -> Result<usize, VmError>;
    fn free_phys_page(&self, pa: usize);
}
