use core::{alloc::Layout, ptr::NonNull};

use alloc::alloc::{alloc_zeroed, dealloc};
use klib::{
    pm::page::mapper::TableAllocator,
    vm::{TABLE_ENTRIES, TTable},
};
use uefi::boot;

#[derive(Debug)]
pub struct UefiTableAlloc;

const LAYOUT: Layout = Layout::new::<TTable<TABLE_ENTRIES>>();

impl TableAllocator for UefiTableAlloc {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let alloc = unsafe { alloc_zeroed(LAYOUT) } as *mut TTable<TABLE_ENTRIES>;
        NonNull::new(alloc).expect("unable to allocate table")
    }
    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        unsafe { dealloc(table.as_ptr() as _, LAYOUT) };
    }
}
