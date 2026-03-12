use core::ptr::NonNull;

use crate::vm::MemoryRegion;

pub trait RawVec<T> {
    fn as_slice(&self) -> &[T];
    fn into_raw_parts(self) -> (NonNull<T>, usize, usize);
    fn from_raw_parts(ptr: NonNull<T>, capacity: usize, len: usize) -> Self;
}

pub trait DynVec<T> {
    fn push(&mut self, object: T);
    fn pop(&mut self) -> Option<T>;
}

#[derive(Debug, Copy, Clone)]
pub struct StaticVec<T> {
    ptr: NonNull<T>,
    capacity: usize,
    len: usize,
}

impl<T> StaticVec<T> {
    pub const fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            capacity: 0,
            len: 0,
        }
    }
}

impl<T> RawVec<T> for StaticVec<T> {
    fn from_raw_parts(ptr: NonNull<T>, capacity: usize, len: usize) -> Self {
        Self { ptr, capacity, len }
    }

    fn into_raw_parts(self) -> (NonNull<T>, usize, usize) {
        let ptr = self.ptr;
        let cap = self.capacity;
        let len = self.len;

        core::mem::forget(self);

        (ptr, cap, len)
    }

    fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
        }
    }
}

impl StaticVec<MemoryRegion> {
    pub fn remove_containing(&mut self, addr: usize) -> Option<MemoryRegion> {
        let index = (0..self.len).find(|&i| {
            let reg = unsafe { *self.ptr.as_ptr().add(i) };
            addr >= reg.base && addr < (reg.base + reg.size)
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
}
