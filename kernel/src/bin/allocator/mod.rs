extern crate alloc;

use core::ptr::NonNull;
use klib::pm::page::mapper::{AddressTranslator as AT, TableAllocator};
use klib::vm::{TABLE_ENTRIES, TTable, dmap_addr_to_phys, phys_addr_to_dmap};

use super::KALLOCATOR;

#[derive(Debug)]
pub struct KernelPTAllocator;

#[derive(Debug)]
pub struct KernelAddressTranslator;

impl TableAllocator for KernelPTAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        unsafe {
            let raw_ptr = KALLOCATOR.alloc_page() as *mut TTable<TABLE_ENTRIES>;

            core::ptr::write(raw_ptr as *mut [u64; TABLE_ENTRIES], [0u64; TABLE_ENTRIES]);

            NonNull::new(raw_ptr).expect("null pointer from `alloc_page()` on `KALLOCATOR`")
        }
    }

    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        let va = table.as_ptr() as usize;
        KALLOCATOR.free_page(va);
    }
}

impl AT for KernelAddressTranslator {
    fn dmap_to_phys<T>(virt: *mut T) -> u64 {
        dmap_addr_to_phys(virt as _)
    }
    fn phys_to_dmap<T>(phys: u64) -> *mut T {
        phys_addr_to_dmap(phys) as _
    }
}
