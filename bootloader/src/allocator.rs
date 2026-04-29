use core::{alloc::Layout, ptr::NonNull};

use alloc::alloc::{alloc_zeroed, dealloc};
use klib::{
    pm::page::mapper::TableAllocator,
    vm::{PAGE_SIZE, TABLE_ENTRIES, TTable, align_down, align_up},
};
use log::debug;
use uefi::boot::{self, MemoryType, PAGE_SIZE as UEFI_PS};

#[derive(Debug)]
pub struct UefiTableAlloc;

const SIZE: usize = size_of::<TTable<TABLE_ENTRIES>>();

impl TableAllocator for UefiTableAlloc {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let pages = SIZE / UEFI_PS;
        let extra = PAGE_SIZE / UEFI_PS;

        let alloc = boot::allocate_pages(
            boot::AllocateType::AnyPages,
            MemoryType::LOADER_CODE,
            pages + extra,
        )
        .expect("alloc fail")
        .as_ptr();

        let start = alloc as usize;
        let start = align_up(start, PAGE_SIZE);

        let ptr = start as *mut TTable<TABLE_ENTRIES>;

        // zero
        unsafe { ptr.write(TTable::new()) };

        NonNull::new(ptr).expect("unable to allocate table")
    }

    // more trouble than it's worth
    fn free_table(&self, _table: NonNull<TTable<TABLE_ENTRIES>>) {
        unimplemented!()
    }
}
