use crate::{IoError, buffer::IoBuffer};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DmaDirection {
    ToDevice,
    FromDevice,
}

#[derive(Debug, Copy, Clone)]
pub struct DmaSegment {
    /// address in the device's DMA address space
    pub address: u64,
    pub len: usize,
}

pub trait DmaMapping {
    fn segments(&self) -> &[DmaSegment];
}

pub trait DmaMapper {
    type Mapping: DmaMapping;

    fn map(&self, buffer: IoBuffer, direction: DmaDirection) -> Result<Self::Mapping, IoError>;
}
