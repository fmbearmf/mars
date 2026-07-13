use core::{alloc::Layout, fmt::Debug, ptr::NonNull};

use alloc::alloc::{alloc, dealloc};
use log::trace;

use crate::vm::align_up;

pub struct Stack {
    ptr: NonNull<u8>,
    layout: Layout,
}

impl Debug for Stack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AllocatedStack")
            .field("pointer", &self.ptr)
            .field("layout", &self.layout)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    BadAlignment,
    BadSize,
    Layout,
    AllocFail,
}

impl Stack {
    pub fn new(size: usize, alignment: usize) -> Result<Self, self::Error> {
        if !alignment.is_power_of_two() {
            return Err(self::Error::BadAlignment);
        }

        if !size.is_power_of_two() {
            return Err(self::Error::BadSize);
        }

        let layout = Layout::from_size_align(size, alignment).map_err(|_| self::Error::Layout)?;

        unsafe {
            let ptr = alloc(layout);
            let ptr = NonNull::new(ptr).ok_or(self::Error::AllocFail)?;
            Ok(Self { ptr, layout })
        }
    }

    /// highest address
    pub fn top(&self) -> *mut u8 {
        unsafe { self.ptr.as_ptr().add(self.layout.size()) }
    }

    /// lowest address
    pub fn bottom(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    pub fn as_ptr_range(&self) -> core::ops::Range<*mut u8> {
        self.bottom()..self.top()
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        trace!("dropping stack: {:?}", self);
        unsafe { dealloc(self.ptr.as_ptr(), self.layout) }
    }
}
