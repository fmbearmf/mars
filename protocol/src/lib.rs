#![no_std]

use core::ptr::NonNull;

use klib::vm::{TABLE_ENTRIES, TTable};
use uefi::mem::memory_map::MemoryMapOwned;
use uefi_raw::table::system::SystemTable;

#[derive(Debug)]
pub struct BootInfo {
    /// physical load address of the kernel
    pub kernel_load_physical_address: usize,

    /// size of the kernel in bytes
    pub kernel_size: usize,

    /// the TTBR0 that the kernel should load, if any
    pub page_table_root: Option<*const TTable<TABLE_ENTRIES>>,

    /// serial uart
    pub serial_uart_address: usize,

    /// UEFI memory map
    pub memory_map: MemoryMapOwned,

    /// UEFI system table
    pub system_table_raw: NonNull<SystemTable>,
}
