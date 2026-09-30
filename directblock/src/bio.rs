#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

/// physical memory segment for DMA
#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct BioVec {
    pub dma_addr: PhysAddr,
    pub len: u32,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct DiscardRange {
    pub slba: u64,
    pub num_blocks: u32,
    pub flags: u32,
}
