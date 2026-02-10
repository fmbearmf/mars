#![no_std]

use uefi::mem::memory_map::{MemoryMapIter, MemoryMapMeta, MemoryMapMut, MemoryMapOwned};

#[derive(Debug)]
pub struct BootInfo {
    /// physical load address of the kernel
    pub kernel_load_physical_address: usize,

    /// size of the kernel in bytes
    pub kernel_size: usize,

    /// memory map
    pub memory_map: MemoryMapOwned,
}
