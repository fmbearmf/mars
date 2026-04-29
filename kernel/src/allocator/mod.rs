use core::ptr::NonNull;
use klib::pm::page::mapper::{AddressTranslator as AT, TableAllocator};
use klib::vm::page_allocator::DmapPageAllocator;
use klib::vm::{TABLE_ENTRIES, TTable, dmap_addr_to_phys, phys_addr_to_dmap};

use super::KALLOCATOR;

#[derive(Debug)]
pub struct KernelPTAllocator;

#[derive(Debug)]
pub struct KernelAddressTranslator;

impl TableAllocator for KernelPTAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        unsafe {
            let raw_ptr: usize = KALLOCATOR.alloc_dmap_page().expect("page alloc fail");
            let raw_ptr = raw_ptr as *mut TTable<TABLE_ENTRIES>;

            unsafe { (raw_ptr as *mut [u64; TABLE_ENTRIES]).write_bytes(0, 1) };

            NonNull::new(raw_ptr).expect("null pointer from `alloc_page()` on `KALLOCATOR`")
        }
    }

    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        let va = table.as_ptr() as usize;
        KALLOCATOR.free_page(va);
    }
}

impl AT for KernelAddressTranslator {
    fn dmap_to_phys(&self, virt: *mut u8) -> usize {
        dmap_addr_to_phys(virt as _) as _
    }
    fn phys_to_dmap(&self, phys: usize) -> *mut u8 {
        phys_addr_to_dmap(phys as _) as _
    }
}
