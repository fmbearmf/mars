use core::{fmt, marker::PhantomData, num::NonZeroUsize, ptr::NonNull};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum AlignmentError {
    Zero,
    NotPowerOfTwo,
}

/// power-of-two alignment constraint
pub trait Alignment: Copy + Clone + Send + Sync + 'static {
    const ALIGN_BYTES: usize;

    const VALID: () = {
        assert!(Self::ALIGN_BYTES > 0, "ALIGN_BYTES must be > 0");
        assert!(
            Self::ALIGN_BYTES.is_power_of_two(),
            "ALIGN_BYTES must be a power of two"
        );
    };

    const VALID_RESULT: Result<(), AlignmentError> = {
        if Self::ALIGN_BYTES == 0 {
            Err(AlignmentError::Zero)
        } else if !Self::ALIGN_BYTES.is_power_of_two() {
            Err(AlignmentError::NotPowerOfTwo)
        } else {
            Ok(())
        }
    };
}

#[derive(Debug, Copy, Clone)]
pub struct Align4K;
impl Alignment for Align4K {
    const ALIGN_BYTES: usize = 4096;
}

#[derive(Debug, Copy, Clone)]
pub struct Align512;
impl Alignment for Align512 {
    const ALIGN_BYTES: usize = 512;
}

#[derive(Debug, Copy, Clone)]
pub struct Align64;
impl Alignment for Align64 {
    const ALIGN_BYTES: usize = 64;
}

/// marker; specify memory access permissions relative to DMA
pub trait BufferAccess: 'static {
    type View<'a>;
    unsafe fn make_view<'a>(ptr: *mut u8, len: usize) -> Self::View<'a>;
}
pub struct ReadOnly;
pub struct WriteOnly;

impl BufferAccess for ReadOnly {
    type View<'a> = &'a [u8];

    unsafe fn make_view<'a>(ptr: *mut u8, len: usize) -> Self::View<'a> {
        unsafe { core::slice::from_raw_parts(ptr, len) }
    }
}
impl BufferAccess for WriteOnly {
    type View<'a> = &'a mut [u8];

    unsafe fn make_view<'a>(ptr: *mut u8, len: usize) -> Self::View<'a> {
        unsafe { core::slice::from_raw_parts_mut(ptr, len) }
    }
}

/// non-data requirements (e.g. flush, sync, barriers)
pub struct NoData;
impl BufferAccess for NoData {
    type View<'a> = ();

    unsafe fn make_view<'a>(_ptr: *mut u8, _len: usize) -> Self::View<'a> {}
}

/// zero-copy contiguous memory region loaned to directblock
pub struct AlignedBuffer<'a, A: Alignment, Access: BufferAccess> {
    pub(crate) ptr: NonNull<u8>,
    pub(crate) len: NonZeroUsize,
    pub(crate) _align: PhantomData<A>,
    pub(crate) _access: PhantomData<Access>,
    pub(crate) _lifetime: PhantomData<&'a mut [u8]>,
}

unsafe impl<'a, A: Alignment, Access: BufferAccess> Send for AlignedBuffer<'a, A, Access> {}
unsafe impl<'a, A: Alignment, Access: BufferAccess> Sync for AlignedBuffer<'a, A, Access> {}

impl<'a, A: Alignment, Access: BufferAccess> fmt::Debug for AlignedBuffer<'a, A, Access> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlignedBuffer")
            .field("ptr", &self.ptr)
            .field("len", &self.len)
            .field("align_bytes", &A::ALIGN_BYTES)
            .finish()
    }
}

impl<'a, A: Alignment> AlignedBuffer<'a, A, ReadOnly> {
    const _ALIGNED: () = A::VALID;

    /// create device-readable (CPU R/O) buffer for write/trim
    pub fn from_slice_ro(slice: &'a [u8]) -> Option<Self> {
        // enforce alignment, but don't panic
        A::VALID_RESULT.ok()?;

        let len = NonZeroUsize::new(slice.len())?;
        if (slice.as_ptr() as usize | slice.len()) & (A::ALIGN_BYTES - 1) != 0 {
            return None;
        }

        NonNull::new(slice.as_ptr() as *mut u8).map(|ptr| Self {
            ptr,
            len,
            _align: PhantomData,
            _access: PhantomData,
            _lifetime: PhantomData,
        })
    }

    pub fn as_slice(&self) -> &'a [u8] {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len.get()) }
    }
}

impl<'a, A: Alignment> AlignedBuffer<'a, A, WriteOnly> {
    const _ALIGNED: () = A::VALID;

    /// construct device-writable buffer for reading
    pub fn from_slice_wo(slice: &'a mut [u8]) -> Option<Self> {
        // enforce alignment, but don't panic
        A::VALID_RESULT.ok()?;

        let len = NonZeroUsize::new(slice.len())?;
        if (slice.as_ptr() as usize | slice.len()) & (A::ALIGN_BYTES - 1) != 0 {
            return None;
        }

        NonNull::new(slice.as_mut_ptr()).map(|ptr| Self {
            ptr,
            len,
            _align: PhantomData,
            _access: PhantomData,
            _lifetime: PhantomData,
        })
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.as_mut_ptr(), self.len.get()) }
    }
}

impl<'a, A: Alignment, Access: BufferAccess> AlignedBuffer<'a, A, Access> {
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    pub fn len(&self) -> usize {
        self.len.get()
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}
