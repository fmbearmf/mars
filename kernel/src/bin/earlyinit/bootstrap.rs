use core::cell::UnsafeCell;

use uefi::mem::memory_map::MemoryMapOwned;

pub struct EarlyPtAllocator {
    mmap: UnsafeCell<*mut MemoryMapOwned>,
}

impl EarlyPtAllocator {
    pub fn new(mmap: &mut MemoryMapOwned) -> Self {
        Self {
            mmap: UnsafeCell::new(mmap as *mut _),
        }
    }
}
