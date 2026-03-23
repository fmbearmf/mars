use core::{
    cell::RefCell,
    mem::forget,
    ptr::{self, NonNull},
};

use crate::{
    vec::{DynVec, PMVec, RawVec, StaticVec},
    vm::{MemoryRegion, MemoryRegionType, align_up},
};

use super::{
    super::{PAGE_MASK, PAGE_SIZE, TABLE_ENTRIES, TTable, map::TableAllocator},
    PageAllocator,
};

pub struct KernelPTAllocator<'a> {
    pa: &'a PageAllocator,
    regions: RefCell<StaticVec<MemoryRegion>>,
}

impl<'a> KernelPTAllocator<'a> {
    pub const fn new(pa: &'a PageAllocator, regions: StaticVec<MemoryRegion>) -> Self {
        Self {
            pa,
            regions: RefCell::new(regions),
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

        if let Some(_reg) = self.regions.borrow_mut().remove_containing(addr) {
            return;
        }

        self.pa.free_pages(table.as_ptr().cast());
    }
}

pub struct PMTableAllocator {
    pub free_regions: RefCell<PMVec<MemoryRegion>>,
}

impl PMTableAllocator {
    pub const fn new(regions: PMVec<MemoryRegion>) -> Self {
        Self {
            free_regions: RefCell::new(regions),
        }
    }
}

impl TableAllocator for PMTableAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        let mut regions = self.free_regions.borrow_mut();

        const SIZE: usize = size_of::<TTable<TABLE_ENTRIES>>();
        const ALIGN: usize = align_of::<TTable<TABLE_ENTRIES>>();

        let mut target = None;

        for region in regions.as_slice() {
            let aligned_base = align_up(region.base, ALIGN);

            if !region.is_normal() {
                continue;
            }

            if let Some(end) = aligned_base.checked_add(SIZE) {
                if end <= region.base + region.size {
                    target = Some((aligned_base, region.base));
                    break;
                }
            }
        }

        let (aligned, original) = target.expect("OOM");

        let region = regions
            .remove_containing(original)
            .expect("failed to pop region");

        if aligned > region.base {
            regions.push(MemoryRegion {
                base: region.base,
                size: aligned - region.base,
                region_type: region.region_type,
            });
        }

        let alloc_end = aligned + SIZE;
        let region_end = region.base + region.size;

        if region_end > alloc_end {
            regions.push(MemoryRegion {
                base: alloc_end,
                size: region_end - alloc_end,
                region_type: region.region_type,
            });
        }

        regions.compact();

        let ptr = NonNull::new(aligned as *mut TTable<TABLE_ENTRIES>).expect("null ptr");
        let ptr_ish = ptr.as_ptr() as *mut [u64; TABLE_ENTRIES];
        unsafe {
            ptr_ish.write([0; TABLE_ENTRIES]);
        };
        //panic!("ptr: {:#x} is {:#x?}", ptr_ish as usize, unsafe {
        //    ptr_ish.read()
        //});
        //panic!("glub");
        ptr
    }

    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>) {
        let mut regions = self.free_regions.borrow_mut();

        regions.push(MemoryRegion {
            base: table.as_ptr() as usize,
            size: size_of::<TTable<TABLE_ENTRIES>>(),
            region_type: MemoryRegionType::PageTable,
        });

        regions.compact();
    }
}
