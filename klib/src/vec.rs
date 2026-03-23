use core::{
    cell::UnsafeCell,
    mem::forget,
    ptr::{NonNull, copy_nonoverlapping},
    slice::{from_raw_parts, from_raw_parts_mut},
};

use crate::vm::{MemoryRegion, align_up};

const PM_POOL_SIZE: usize = size_of::<MemoryRegion>() * 1024;

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

#[repr(C, align(16))]
struct PmPool {
    buf: UnsafeCell<[u8; PM_POOL_SIZE]>,
    offset: UnsafeCell<usize>,
}

unsafe impl Sync for PmPool {}

static PM_POOL: PmPool = PmPool {
    buf: UnsafeCell::new([0; PM_POOL_SIZE]),
    offset: UnsafeCell::new(0),
};

fn pm_alloc<T>(count: usize) -> NonNull<T> {
    if size_of::<T>() == 0 {
        return NonNull::dangling();
    }

    let size = count
        .checked_mul(size_of::<T>())
        .expect("allocation size OOB");
    let align = align_of::<T>();

    unsafe {
        let offset_ptr = PM_POOL.offset.get();
        let offset = *offset_ptr;

        let pool_ptr = PM_POOL.buf.get() as *mut u8;
        let pool_addr = pool_ptr as usize;

        let current_addr = pool_addr.checked_add(offset).expect("address OOB");
        let aligned_addr = align_up(current_addr, align);
        let pad = aligned_addr - current_addr;

        let start = offset.checked_add(pad).expect("padding OOB");
        let new_offset = start.checked_add(size).expect("size OOB");

        if new_offset > PM_POOL_SIZE {
            panic!(
                "PMVec out of memory. req {} bytes, capacity {}",
                size, PM_POOL_SIZE
            );
        }

        *offset_ptr = new_offset;

        let ptr = pool_ptr.add(start) as *mut T;
        NonNull::new(ptr).expect("ptr shouldn't be null")
    }
}

fn pm_free<T>(ptr: NonNull<T>, count: usize) {
    if count == 0 || size_of::<T>() == 0 {
        return;
    }

    let size = count.checked_mul(size_of::<T>()).expect("free size OOB");
    let ptr_addr = ptr.as_ptr() as usize;

    unsafe {
        let pool_addr = PM_POOL.buf.get() as usize;
        let pool_end = pool_addr.checked_add(PM_POOL_SIZE).unwrap();

        if ptr_addr >= pool_addr && ptr_addr < pool_end {
            let alloc_start = ptr_addr - pool_addr;
            let expected_off = alloc_start.checked_add(size).unwrap();

            let offset_ptr = PM_POOL.offset.get();
            if *offset_ptr == expected_off {
                *offset_ptr = alloc_start;
            }
        }
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

        let new_ptr: NonNull<T> = pm_alloc(new_cap);

        if self.capacity > 0 {
            unsafe {
                copy_nonoverlapping(self.ptr.as_ptr(), new_ptr.as_ptr(), self.len);
                pm_free(self.ptr, self.capacity);
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
        if self.capacity > 0 {
            pm_free(self.ptr, self.capacity);
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
