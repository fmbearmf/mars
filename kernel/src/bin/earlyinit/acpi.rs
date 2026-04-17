use core::{range::Range, slice};

use klib::acpi::rsdp::find_rsdp_in_slice;
use uefi::{
    boot::{MemoryType, PAGE_SIZE as UEFI_PAGE_SIZE},
    mem::memory_map::{MemoryMap, MemoryMapOwned},
};

pub fn discover_uart_uefi(mmap: &MemoryMapOwned) {
    for region in mmap.entries() {
        match region.ty {
            MemoryType::ACPI_RECLAIM => {
                let slice: &[u8] = unsafe {
                    slice::from_raw_parts(
                        region.phys_start as *const _,
                        region.page_count as usize * UEFI_PAGE_SIZE,
                    )
                };

                let rsdp = find_rsdp_in_slice(slice);
            }
            _ => {}
        }
    }
}
