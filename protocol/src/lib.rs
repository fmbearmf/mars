#![no_std]

use core::ptr::NonNull;

use klib::{
    vec::StaticVec,
    vm::{MemoryRegion, TABLE_ENTRIES, TTable},
};
use uefi::mem::memory_map::{MemoryMapIter, MemoryMapMeta, MemoryMapMut, MemoryMapOwned};

#[derive(Debug)]
pub struct BootInfo {
    /// physical load address of the kernel
    pub kernel_load_physical_address: usize,

    /// size of the kernel in bytes
    pub kernel_size: usize,

    /// serial uart
    pub serial_uart_address: usize,

    /// memory map
    pub memory_map: MemoryMapOwned,

    /// root (l0) ttbr1 page table
    pub root_pt: NonNull<TTable<TABLE_ENTRIES>>,

    /// memory regions (whose ownership needs to be passed to the kernel)
    pub kernel_regions: StaticVec<MemoryRegion>,
}
