#![no_std]

use core::ptr::NonNull;

use klib::{
    vec::StaticVec,
    vm::{MemoryRegion, TABLE_ENTRIES, TTable},
};
use uefi::mem::memory_map::MemoryMapOwned;

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
}
