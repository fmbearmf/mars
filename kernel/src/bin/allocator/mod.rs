extern crate alloc;

use alloc::alloc::{alloc, dealloc, handle_alloc_error};
use core::{alloc::Layout, ptr::NonNull};
use klib::pm::page::mapper::TableAllocator;
use klib::vm::page_allocator::PhysicalPageAllocator;
use klib::vm::{TABLE_ENTRIES, TTable, dmap_addr_to_phys, phys_addr_to_dmap};

use super::{KALLOCATOR, earlycon_writeln};

pub struct KernelPTAllocator;

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

    fn phys_to_virt(&self, phys: u64) -> *mut TTable<TABLE_ENTRIES> {
        phys_addr_to_dmap(phys) as *mut TTable<TABLE_ENTRIES>
    }

    fn virt_to_phys(&self, virt: *mut TTable<TABLE_ENTRIES>) -> u64 {
        dmap_addr_to_phys(virt as u64)
    }
}
