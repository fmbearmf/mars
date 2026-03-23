use core::{
    cell::RefCell,
    ptr::{self, NonNull},
};

use crate::{vec::StaticVec, vm::MemoryRegion};

use super::{
    super::{PAGE_MASK, PAGE_SIZE, TABLE_ENTRIES, TTable, map::TableAllocator},
    PageAllocator,
};

pub struct KernelPTAllocator<'a> {
    pa: &'a PageAllocator,
    uefi_regions: RefCell<StaticVec<MemoryRegion>>,
}

impl<'a> KernelPTAllocator<'a> {
    pub const fn new(pa: &'a PageAllocator, uefi_regions: StaticVec<MemoryRegion>) -> Self {
        Self {
            pa,
            uefi_regions: RefCell::new(uefi_regions),
        }
    }
}

impl<'a> TableAllocator for KernelPTAllocator<'a> {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let page = self.pa.alloc_page();
        if page.is_null() {
            panic!("KernelPTAllocator: out of pages");
        }

        let addr = page as usize;
        debug_assert_eq!(addr & PAGE_MASK, 0, "allocated page isn't page aligned!");

        let table_size = size_of::<TTable<TABLE_ENTRIES>>();
        debug_assert!(
            table_size <= PAGE_SIZE,
            "TTable size ({}) larger than PAGE_SIZE ({})",
            table_size,
            PAGE_SIZE
        );

        unsafe {
            ptr::write_bytes(page as *mut u8, 0, PAGE_SIZE);

            let table_ptr = page as *mut TTable<TABLE_ENTRIES>;
            ptr::write(table_ptr, TTable::new());

            NonNull::new(table_ptr).expect("page ptr non-null")
        }
    }

    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        let addr = table.as_ptr() as usize;

        if let Some(_reg) = self.uefi_regions.borrow_mut().remove_containing(addr) {
            return;
        }

        self.pa.free_pages(table.as_ptr().cast());
    }
}
