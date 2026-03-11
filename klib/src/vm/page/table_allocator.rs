use core::ptr::{self, NonNull};

use super::{
    super::{PAGE_MASK, PAGE_SIZE, TABLE_ENTRIES, TTable, map::TableAllocator},
    PageAllocator,
};

pub struct KernelPTAllocator<'a> {
    pa: &'a PageAllocator,
}

impl<'a> KernelPTAllocator<'a> {
    pub const fn new(pa: &'a PageAllocator) -> Self {
        Self { pa }
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
}
