use core::{
    cell::UnsafeCell,
    mem::forget,
    ptr::{NonNull, copy_nonoverlapping},
    slice::{from_raw_parts, from_raw_parts_mut},
};

use super::{
    pm::PM_POOL,
    vm::{MemoryRegion, align_up},
};

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct StaticVec<T> {
    ptr: NonNull<T>,
    capacity: usize,
    len: usize,
}

pub trait RawVec<T> {
    fn as_slice(&self) -> &[T];
    fn into_raw_parts(self) -> (NonNull<T>, usize, usize);
    fn from_raw_parts(ptr: NonNull<T>, capacity: usize, len: usize) -> Self;
}

pub trait DynVec<T> {
    fn push(&mut self, object: T);
    fn pop(&mut self) -> Option<T>;
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

#[derive(Debug)]
pub struct PMVec<T> {
    ptr: NonNull<T>,
    capacity: usize,
    len: usize,
}

impl<T> PMVec<T> {
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
            self.capacity.checked_mul(2).expect("capacity OOB")
        };

        if size_of::<T>() == 0 {
            self.capacity = new_cap;
            return;
        }

        let new_ptr: NonNull<T> = PM_POOL.alloc();
        for _ in 1..new_cap {
            let _: NonNull<T> = PM_POOL.alloc();
        }

        unsafe {
            copy_nonoverlapping(self.ptr.as_ptr(), new_ptr.as_ptr(), self.len);
            for i in (0..self.capacity).rev() {
                PM_POOL.free(NonNull::new_unchecked(self.ptr.as_ptr().add(i)));
            }
        }

        self.ptr = new_ptr;
        self.capacity = new_cap;
    }

    pub fn extend(&mut self, other: PMVec<T>) {
        let req = self.len.checked_add(other.len).expect("len OOB");

        while req > self.capacity {
            self.grow();
        }

        unsafe {
            copy_nonoverlapping(
                other.ptr.as_ptr(),
                self.ptr.as_ptr().add(self.len),
                other.len,
            );
        }
        self.len = req;

        _ = other.into_raw_parts();
    }
}

impl<T> DynVec<T> for PMVec<T> {
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

impl<T> Drop for PMVec<T> {
    fn drop(&mut self) {
        if self.capacity > 0 && size_of::<T>() > 0 {
            for i in (0..self.capacity).rev() {
                unsafe {
                    PM_POOL.free(NonNull::new_unchecked(self.ptr.as_ptr().add(i)));
                }
            }
        }
    }
}

impl<T> RawVec<T> for PMVec<T> {
    fn as_slice(&self) -> &[T] {
        if self.len == 0 {
            &[]
        } else {
            unsafe { from_raw_parts(self.ptr.as_ptr(), self.len) }
        }
    }

    fn into_raw_parts(mut self) -> (NonNull<T>, usize, usize) {
        let ptr = self.ptr;
        let cap = self.capacity;
        let len = self.len;

        self.capacity = 0;
        self.len = 0;

        forget(self);

        (ptr, cap, len)
    }

    fn from_raw_parts(ptr: NonNull<T>, capacity: usize, len: usize) -> Self {
        Self { ptr, capacity, len }
    }
}

impl PMVec<MemoryRegion> {
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

impl<T: Copy> PMVec<T> {
    pub fn extend_from_slice(&mut self, slice: &[T]) {
        let additional = slice.len();
        if additional == 0 {
            return;
        }

        let required = self.len.checked_add(additional).expect("length OOB");

        while required > self.capacity {
            self.grow();
        }

        unsafe {
            copy_nonoverlapping(slice.as_ptr(), self.ptr.as_ptr().add(self.len), additional);
        }

        self.len = required;
    }
}
