extern crate alloc;

use alloc::alloc::{alloc, alloc_zeroed, dealloc, handle_alloc_error};
use core::{
    alloc::Layout,
    ptr::{NonNull, write_bytes, write_volatile},
};
use klib::vm::{TABLE_ENTRIES, TTable, dmap_addr_to_phys, map::TableAllocator, phys_addr_to_dmap};

use super::{busy_loop_ret, earlycon_writeln};

pub struct KernelPTAllocator;

impl TableAllocator for KernelPTAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let layout = Layout::new::<TTable<TABLE_ENTRIES>>();

        unsafe {
            let raw_ptr = alloc(layout) as *mut TTable<TABLE_ENTRIES>;

            core::ptr::write(raw_ptr as *mut [u64; TABLE_ENTRIES], [0u64; TABLE_ENTRIES]);

            NonNull::new(raw_ptr).unwrap_or_else(|| handle_alloc_error(layout))
        }
    }

    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        let layout = Layout::new::<TTable<TABLE_ENTRIES>>();

        let ptr = table.as_ptr() as *mut u8;

        unsafe {
            dealloc(ptr, layout);
        }
    }

    fn phys_to_virt(&self, phys: u64) -> *mut TTable<TABLE_ENTRIES> {
        phys_addr_to_dmap(phys) as *mut TTable<TABLE_ENTRIES>
    }

    fn virt_to_phys(&self, virt: *mut TTable<TABLE_ENTRIES>) -> u64 {
        dmap_addr_to_phys(virt as u64)
    }
}
