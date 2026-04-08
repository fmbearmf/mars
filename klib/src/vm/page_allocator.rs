use super::VmError;

pub trait PhysicalPageAllocator {
    fn alloc_page(&self) -> Result<usize, VmError>;
    fn free_page(&self, pa: usize);
}
