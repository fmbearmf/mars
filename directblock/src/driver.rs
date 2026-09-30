use core::{fmt, marker::PhantomData};

use crate::{
    align::BufferAccess,
    bio::BioVec,
    descriptor::{DeviceStatus, Lba},
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct RequestTag(pub u16);

impl RequestTag {
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    pub fn as_u16(self) -> u16 {
        self.0
    }
}

impl fmt::Display for RequestTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct UserData(pub u64);

impl UserData {
    pub const EMPTY: Self = Self(0);

    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SglError {
    EmptyList,
    SegmentTooLarge,
    TotalLengthOverflow,
    MisalignedLength { len: u32, align: u32 },
}

impl fmt::Display for SglError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyList => write!(f, "scatter-gather list can't be empty"),
            Self::SegmentTooLarge => write!(f, "scatter-gather entry exceeds maximum segment size"),
            Self::TotalLengthOverflow => write!(f, "total byte length overflows usize"),
            Self::MisalignedLength { len, align } => {
                write!(
                    f,
                    "entry len {} is not a multiple of alignment {}",
                    len, align
                )
            }
        }
    }
}

impl core::error::Error for SglError {}

#[derive(Debug, Copy, Clone)]
pub struct ScatterGatherList<'a, Access: BufferAccess> {
    entries: &'a [BioVec],
    total_bytes: usize,
    _access: PhantomData<Access>,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub enum RequestOp {
    Read,
    Write,
    Flush,
    Discard,
    ZoneAppend,
}

#[repr(C, align(64))]
#[derive(Debug, Copy, Clone)]
pub struct BlockRequest {
    pub tag: u16,
    pub op: RequestOp,
    pub lba: Lba,
    pub block_count: u32,
    pub sgl_ptr: *const BioVec,
    pub sgl_len: u16,
    pub user_data: u64,
}

unsafe impl Send for BlockRequest {}

#[derive(Debug, Copy, Clone)]
pub struct Completion {
    pub tag: u16,
    pub user_data: u64,
    pub status: DeviceStatus,
    pub bytes_transferred: usize,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SubmissionError {
    QueueFull,
    DeviceFailure,
}

pub trait BlockQueue: Send + Sync + 'static {
    fn submit(&self, req: BlockRequest) -> Result<(), SubmissionError>;

    /// reap completions into the provided slice, returning count reaped
    fn poll_completions(&self, completions: &mut [Completion]) -> usize;
}
