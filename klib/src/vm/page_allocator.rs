use core::ptr::NonNull;

use crate::{
    pm::page::mapper::TableAllocator,
    vm::{KALLOCATOR, TABLE_ENTRIES, TTable},
};

use super::VmError;

pub trait PhysicalPageAllocator {
    fn alloc_phys_page(&self) -> Result<usize, VmError>;
    fn free_phys_page(&self, pa: usize);
}

pub trait DmapPageAllocator {
    fn alloc_dmap_page(&self) -> Result<usize, VmError>;
    fn free_dmap_page(&self, pa: usize);
}

#[derive(Debug)]
pub struct KernelPTAllocator;

impl TableAllocator for KernelPTAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let raw_ptr: usize = KALLOCATOR.alloc_dmap_page().expect("page alloc fail");
        let raw_ptr = raw_ptr as *mut TTable<TABLE_ENTRIES>;

        unsafe { (raw_ptr as *mut [u64; TABLE_ENTRIES]).write_bytes(0, 1) };

        NonNull::new(raw_ptr).expect("null pointer from `alloc_page()` on `KALLOCATOR`")
    }

    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        let va = table.as_ptr() as usize;
        KALLOCATOR.free_page(va);
    }
}
