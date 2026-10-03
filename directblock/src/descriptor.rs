use core::{
    fmt,
    marker::PhantomData,
    num::{NonZeroU64, NonZeroUsize},
    ptr::NonNull,
};

use crate::{
    align::{Align512, AlignedBuffer, Alignment, BufferAccess, NoData},
    op::{self, BlockOperation},
    state,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Lba(pub u64);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct BlockCount(pub NonZeroU64);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceStatus {
    Success = 0,
    InvalidLba = 1,
    MediaError = 2,
    HardwareFault = 3,
    ZoneFull = 4,
    DmaAborted = 5,
}

#[repr(C, align(64))]
pub struct Descriptor<'a, Op: BlockOperation, A: Alignment, State> {
    pub lba: Lba,
    pub block_count: BlockCount,
    pub user_data: u64,
    pub flags: u32,
    pub status: u8,

    buffer_ptr: u64,
    buffer_len: u64,

    pub _marker: PhantomData<(&'a (), Op, A, State)>,
}

// safe to send is the view is safe to send
unsafe impl<'a, Op: BlockOperation, A: Alignment, State> Send for Descriptor<'a, Op, A, State> where
    <Op::RequiredAccess as BufferAccess>::View<'a>: Send
{
}

impl<'a, Op: BlockOperation, A: Alignment, State> fmt::Debug for Descriptor<'a, Op, A, State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Descriptor")
            .field("lba", &self.lba)
            .field("block_count", &self.block_count)
            .field("user_data", &self.user_data)
            .field("flags", &self.flags)
            .field("status", &self.status)
            .field("buffer_ptr", &self.buffer_ptr)
            .field("buffer_len", &self.buffer_len)
            .finish()
    }
}

impl<Op: BlockOperation, A: Alignment> Descriptor<'static, Op, A, state::Draft> {
    pub fn new(lba: Lba, block_count: BlockCount, user_data: u64) -> Self {
        Self {
            lba,
            block_count,
            user_data,
            flags: 0,
            status: 0,
            buffer_ptr: 0,
            buffer_len: 0,
            _marker: PhantomData,
        }
    }

    /// attaches an `AlignedBuffer`, transferring its owned memory
    pub fn attach_buffer<'a>(
        self,
        buf: AlignedBuffer<'a, A, Op::RequiredAccess>,
    ) -> Descriptor<'a, Op, A, state::Bound<'a>> {
        Descriptor {
            lba: self.lba,
            block_count: self.block_count,
            user_data: self.user_data,
            flags: self.flags,
            status: 0,
            buffer_ptr: buf.as_ptr() as usize as u64,
            buffer_len: buf.len() as u64,
            _marker: PhantomData,
        }
    }
}

impl<Op: BlockOperation<RequiredAccess = NoData>, A: Alignment>
    Descriptor<'static, Op, A, state::Draft>
{
    /// non-data commands have no need for an allocation
    pub fn finalize_without_buffer(self) -> Descriptor<'static, Op, A, state::Bound<'static>> {
        Descriptor {
            lba: self.lba,
            block_count: self.block_count,
            user_data: self.user_data,
            flags: self.flags,
            status: 0,
            buffer_ptr: 0,
            buffer_len: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, Op: BlockOperation, A: Alignment> Descriptor<'a, Op, A, state::Bound<'a>> {
    pub fn mark_submitted(self) -> Descriptor<'a, Op, A, state::Submitted<'a>> {
        Descriptor {
            lba: self.lba,
            block_count: self.block_count,
            user_data: self.user_data,
            flags: self.flags,
            status: self.status,
            buffer_ptr: self.buffer_ptr,
            buffer_len: self.buffer_len,
            _marker: PhantomData,
        }
    }
}

impl<'a, Op: BlockOperation, A: Alignment> Descriptor<'a, Op, A, state::Submitted<'a>> {
    pub fn complete(mut self, status: DeviceStatus) -> Descriptor<'a, Op, A, state::Completed> {
        self.status = status as u8;
        Descriptor {
            lba: self.lba,
            block_count: self.block_count,
            user_data: self.user_data,
            flags: self.flags,
            status: self.status,
            buffer_ptr: self.buffer_ptr,
            buffer_len: self.buffer_len,
            _marker: PhantomData,
        }
    }
}

impl<'a, Op: BlockOperation, A: Alignment> Descriptor<'a, Op, A, state::Completed> {
    pub fn is_success(&self) -> bool {
        self.status == DeviceStatus::Success as u8
    }

    pub fn status(&self) -> Result<(), DeviceStatus> {
        match self.status {
            0 => Ok(()),
            1 => Err(DeviceStatus::InvalidLba),
            2 => Err(DeviceStatus::MediaError),
            3 => Err(DeviceStatus::HardwareFault),
            4 => Err(DeviceStatus::ZoneFull),
            _ => Err(DeviceStatus::DmaAborted),
        }
    }

    /// reconstruct the borrowed aligned buffer, if one was attached
    pub fn detach_buffer(self) -> Option<AlignedBuffer<'a, A, Op::RequiredAccess>> {
        if self.buffer_ptr == 0 {
            return None;
        }

        let ptr = NonNull::new(self.buffer_ptr as usize as *mut u8)?;
        let len = NonZeroUsize::new(self.buffer_len as usize)?;

        Some(AlignedBuffer {
            ptr,
            len,
            _align: PhantomData,
            _access: PhantomData,
            _lifetime: PhantomData,
        })
    }
}

const _: () = assert!(core::mem::size_of::<Descriptor<op::Flush, Align512, state::Draft>>() == 64);
const _: () = assert!(core::mem::align_of::<Descriptor<op::Flush, Align512, state::Draft>>() == 64);
