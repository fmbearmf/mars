use core::ptr::NonNull;

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};

use crate::vm::{PAGE_MASK, PAGE_SHIFT, TTENATIVE};

use super::{TABLE_ENTRIES, TTable};

pub trait TableAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>>;
}

pub fn map_region<A: TableAllocator>(
    root: &mut TTable<TABLE_ENTRIES>,
    pa: usize,
    va: usize,
    size: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &A,
) {
    if va & PAGE_MASK != 0 || pa & PAGE_MASK != 0 || size & PAGE_MASK != 0 {
        panic!("addresses AND size must be page aligned");
    }

    //if va & L2_BLOCK_MASK == 0 && size & L2_BLOCK_MASK == 0 {
    //    let num_blocks = size >> L2_BLOCK_SHIFT;

    //    for i in 0..num_blocks {
    //        let vaddr = va + (i << L2_BLOCK_SHIFT);
    //        let paddr = pa + (i << L2_BLOCK_SHIFT);
    //        map_l2_block(root, paddr, vaddr, access, share, uxn, pxn, attr_index);
    //    }

    //    return;
    //} else {
    let num_pages = size >> PAGE_SHIFT;

    for i in 0..num_pages {
        let vaddr = va + (i << PAGE_SHIFT);
        let paddr = pa + (i << PAGE_SHIFT);
        map_page(
            root, paddr, vaddr, access, share, uxn, pxn, attr_index, allocator,
        );
    }
    //}
}

fn map_l2_block<A: TableAllocator>(
    root: &mut TTable<TABLE_ENTRIES>,
    pa: usize,
    va: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &A,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);

    let l0_entry = &mut (root.entries[i0]);

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        let mut z = unsafe { NonNull::new_unchecked(l1_pa as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(table.as_ptr() as u64);

        unsafe { table.as_mut() }
    };

    let l1_entry = &mut (l1_table.entries[i1]);

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        let mut z = unsafe { NonNull::new_unchecked(l2_pa as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(table.as_ptr() as u64);

        unsafe { table.as_mut() }
    };

    let l2_entry = &mut (l2_table.entries[i2]);

    l2_entry.set_is_valid(true);
    l2_entry.set_is_block();
    l2_entry.set_address(pa as u64);
    l2_entry.set_access();
    l2_entry.set_access_permission(access);
    l2_entry.set_shareability(share);
    l2_entry.set_attr_index(attr_index);
    l2_entry.set_executable(!uxn);
    l2_entry.set_privileged_executable(!pxn);
}

#[inline(always)]
fn map_page<A: TableAllocator>(
    root: &mut TTable<TABLE_ENTRIES>,
    pa: usize,
    va: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &A,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);
    let i3 = TTENATIVE::calculate_index(va as u64, 3);

    let l0_entry = &mut (root.entries[i0]);

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        let mut z = unsafe { NonNull::new_unchecked(l1_pa as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(table.as_ptr() as u64);

        unsafe { table.as_mut() }
    };

    let l1_entry = &mut (l1_table.entries[i1]);

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        let mut z = unsafe { NonNull::new_unchecked(l2_pa as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(table.as_ptr() as u64);

        unsafe { table.as_mut() }
    };

    let l2_entry = &mut (l2_table.entries[i2]);

    let l3_table = if l2_entry.is_valid() {
        let l3_pa = l2_entry.address() as *const TTable<TABLE_ENTRIES>;
        let mut table: NonNull<TTable<TABLE_ENTRIES>> =
            unsafe { NonNull::new_unchecked(l3_pa as *mut _) };

        unsafe { table.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l2_entry.set_is_valid(true);
        l2_entry.set_is_table();
        l2_entry.set_address(table.as_ptr() as u64);

        unsafe { table.as_mut() }
    };

    let l3_entry = &mut (l3_table.entries[i3]);

    l3_entry.set_is_valid(true);
    // the block/table bit acts counterintuitively at L3.
    // an L3 PTE must be marked as a table for the MMU to treat it as a PTE
    l3_entry.set_is_table();
    l3_entry.set_address(pa as u64);
    l3_entry.set_access();
    l3_entry.set_access_permission(access);
    l3_entry.set_shareability(share);
    l3_entry.set_attr_index(attr_index);
    l3_entry.set_executable(!uxn);
    l3_entry.set_privileged_executable(!pxn);
}
