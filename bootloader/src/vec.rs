use core::{intrinsics::copy_nonoverlapping, ptr::NonNull, slice::from_raw_parts_mut};

use klib::{
    vec::{DynVec, RawVec, StaticVec},
    vm::MemoryRegion,
};
use uefi::boot::{self, MemoryType};

/// vector backed by `boot::allocate_pool`
pub struct UefiVec<T> {
    ptr: NonNull<T>,
    capacity: usize,
    len: usize,
}

impl<T> UefiVec<T> {
    pub const fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            capacity: 0,
            len: 0,
        }
    }

    fn grow(&mut self) {
        let new_cap = if self.capacity == 0 {
            4
        } else {
            self.capacity * 2
        };
        let new_size = new_cap * size_of::<T>();

        let new_ptr: NonNull<T> = boot::allocate_pool(MemoryType::LOADER_DATA, new_size)
            .unwrap()
            .cast();

        if self.capacity > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(self.ptr.as_ptr(), new_ptr.as_ptr(), self.len);
                _ = boot::free_pool(self.ptr.cast());
            }
        }

        self.ptr = new_ptr;
        self.capacity = new_cap;
    }

    pub fn extend(&mut self, other: UefiVec<T>) {
        while self.len + other.len > self.capacity {
            self.grow();
        }

        unsafe {
            copy_nonoverlapping(
                other.ptr.as_ptr(),
                self.ptr.as_ptr().add(self.len),
                other.len,
            );
        }
        self.len += other.len;

        _ = other.into_raw_parts();
    }
}

impl<T> DynVec<T> for UefiVec<T> {
    fn push(&mut self, object: T) {
        if self.len == self.capacity {
            self.grow();
        }

        unsafe {
            self.ptr.as_ptr().add(self.len).write(object);
        }

        self.len += 1;
    }

    fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            unsafe { Some(self.ptr.as_ptr().add(self.len).read()) }
        }
    }
}

impl<T> Drop for UefiVec<T> {
    fn drop(&mut self) {
        if self.capacity > 0 {
            unsafe { _ = boot::free_pool(self.ptr.cast()) }
        }
    }
}

impl<T> RawVec<T> for UefiVec<T> {
    fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
        }
    }

    fn into_raw_parts(mut self) -> (NonNull<T>, usize, usize) {
        let ptr = self.ptr;
        let cap = self.capacity;
        let len = self.len;

        self.capacity = 0;
        self.len = 0;

        core::mem::forget(self);

        (ptr, cap, len)
    }

    fn from_raw_parts(ptr: NonNull<T>, capacity: usize, len: usize) -> Self {
        Self { ptr, capacity, len }
    }
}

impl UefiVec<MemoryRegion> {
    pub fn remove_containing(&mut self, addr: usize) -> Option<MemoryRegion> {
        let index = (0..self.len).find(|&i| {
            let region = unsafe { *self.ptr.as_ptr().add(i) };
            addr >= region.base && addr < (region.base + region.size)
        })?;

        let reg = unsafe { self.ptr.as_ptr().add(index).read() };

        if index < self.len - 1 {
            unsafe {
                core::ptr::copy(
                    self.ptr.as_ptr().add(index + 1),
                    self.ptr.as_ptr().add(index),
                    self.len - index - 1,
                );
            }
        }
        self.len -= 1;
        Some(reg)
    }

    pub fn compact(&mut self) {
        if self.len < 2 {
            return;
        }

        let slice = unsafe { from_raw_parts_mut(self.ptr.as_ptr(), self.len) };
        slice.sort_unstable_by_key(|r| r.base);

        let mut write_i = 0;

        for read_i in 1..self.len {
            if slice[write_i].can_merge(&slice[read_i]) {
                slice[write_i].merge(slice[read_i]);
            } else {
                write_i += 1;
                if write_i != read_i {
                    slice[write_i] = slice[read_i];
                }
            }
        }

        self.len = write_i + 1;
    }
}
