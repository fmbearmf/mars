use core::{cell::UnsafeCell, ptr::NonNull};

use super::vm::{MemoryRegion, align_up};

#[repr(C, align(16))]
pub struct PmPool<const S: usize> {
    buf: UnsafeCell<[u8; S]>,
    offset: UnsafeCell<usize>,
}

unsafe impl<const S: usize> Sync for PmPool<S> {}

impl<const S: usize> PmPool<S> {
    pub fn alloc<T>(&self) -> NonNull<T> {
        assert_ne!(size_of::<T>(), 0);

        let size = size_of::<T>();
        let align = align_of::<T>();

        unsafe {
            let offset_ptr = self.offset.get();
            let offset = *offset_ptr;

            let pool_ptr = self.buf.get() as *mut u8;
            let pool_addr = pool_ptr as usize;

            let current_addr = pool_addr.checked_add(offset).expect("address OOB");
            let aligned_addr = align_up(current_addr, align);
            let pad = aligned_addr - current_addr;

            let start = offset.checked_add(pad).expect("padding OOB");
            let new_offset = start.checked_add(size).expect("size OOB");

            if new_offset > S {
                panic!(
                    "PM_POOL out of memory. req {} bytes, but capacity = {}",
                    size, S
                )
            }

            *offset_ptr = new_offset;

            let ptr = pool_ptr.add(start) as *mut T;
            NonNull::new(ptr).expect("PM_POOL null ptr")
        }
    }

    pub fn free<T>(&self, ptr: NonNull<T>) {
        assert_ne!(size_of::<T>(), 0);

        let size = size_of::<T>();
        let ptr_addr = ptr.as_ptr() as usize;

        unsafe {
            let pool_addr = self.buf.get() as usize;
            let pool_end = pool_addr.checked_add(S).expect("pool end OOB");

            assert!(ptr_addr >= pool_addr);
            assert!(ptr_addr < pool_end);

            let alloc_start = ptr_addr - pool_addr;
            let expected_off = alloc_start.checked_add(size).expect("offset OOB");

            let offset_ptr = self.offset.get();
            if *offset_ptr == expected_off {
                *offset_ptr = alloc_start;
            }
        }
    }
}

const PM_POOL_SIZE: usize = size_of::<MemoryRegion>() * 1024;

pub static PM_POOL: PmPool<PM_POOL_SIZE> = PmPool {
    buf: UnsafeCell::new([0; PM_POOL_SIZE]),
    offset: UnsafeCell::new(0),
};
