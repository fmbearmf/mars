use core::ptr::NonNull;

use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};

use crate::vm::{
    PAGE_MASK, PAGE_SHIFT, PAGE_SIZE, TABLE_ENTRIES, TTENATIVE, TTable, dmap_addr_to_phys,
    phys_addr_to_dmap,
};

pub trait TableAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>>;
    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>);

    fn phys_to_virt(&self, phys: u64) -> *mut TTable<TABLE_ENTRIES>;
    fn virt_to_phys(&self, virt: *mut TTable<TABLE_ENTRIES>) -> u64;
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
    if va & PAGE_MASK != 0 {
        panic!("VA must be page aligned");
    }

    if pa & PAGE_MASK != 0 {
        panic!("PA must be page aligned");
    }

    if size & PAGE_MASK != 0 {
        panic!("size must be page aligned");
    }

    if size < PAGE_SIZE {
        panic!("can't map less than 1 page!");
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

pub fn unmap_region<A: TableAllocator>(
    root: &mut TTable<TABLE_ENTRIES>,
    va: usize,
    size: usize,
    allocator: &A,
) {
    if va & PAGE_MASK != 0 || size & PAGE_MASK != 0 {
        panic!("addresses AND size must be page aligned");
    }

    let num_pages = size >> PAGE_SHIFT;

    for i in 0..num_pages {
        let vaddr = va + (i << PAGE_SHIFT);
        unmap_page(root, vaddr, allocator);
    }
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
        let l1_va = allocator.phys_to_virt(l1_pa);
        let mut z = unsafe { NonNull::new_unchecked(l1_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(dmap_addr_to_phys(table.as_ptr() as u64));

        unsafe { table.as_mut() }
    };

    let l1_entry = &mut (l1_table.entries[i1]);
    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        let l2_va = allocator.phys_to_virt(l2_pa);
        let mut z = unsafe { NonNull::new_unchecked(l2_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(dmap_addr_to_phys(table.as_ptr() as u64));

        unsafe { table.as_mut() }
    };

    let l2_entry = &mut (l2_table.entries[i2]);

    if l2_entry.is_valid() && l2_entry.is_table() {
        let child_pa = l2_entry.address();
        let child_va = allocator.phys_to_virt(child_pa);
        if child_pa != 0 {
            unsafe {
                let child_ptr = NonNull::new_unchecked(child_va as *mut TTable<TABLE_ENTRIES>);
            }
        }
    }

    l2_entry.set_is_valid(true);
    l2_entry.set_is_block();
    l2_entry.set_address(allocator.virt_to_phys(pa as *mut TTable<TABLE_ENTRIES>));
    l2_entry.set_access();
    l2_entry.set_access_permission(access);
    l2_entry.set_shareability(share);
    l2_entry.set_attr_index(attr_index);
    l2_entry.set_executable(!uxn);
    l2_entry.set_privileged_executable(!pxn);
}

fn unmap_l2_block<A: TableAllocator>(root: &mut TTable<TABLE_ENTRIES>, va: usize, allocator: &A) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);

    let l0_entry = &mut (root.entries[i0]);
    if !l0_entry.is_valid() || !l0_entry.is_table() {
        return;
    }

    let l1_pa = l0_entry.address();
    let l1_va = allocator.phys_to_virt(l1_pa);
    let mut l1_table_ptr = unsafe { NonNull::new_unchecked(l1_va as *mut TTable<TABLE_ENTRIES>) };
    let l1_table = unsafe { l1_table_ptr.as_mut() };
    let l1_entry = &mut (l1_table.entries[i1]);

    if !l1_entry.is_valid() || !l1_entry.is_table() {
        return;
    }

    let l2_pa = l1_entry.address();
    let l2_va = allocator.phys_to_virt(l2_pa);
    let mut l2_table_ptr = unsafe { NonNull::new_unchecked(l2_va as *mut TTable<TABLE_ENTRIES>) };
    let l2_table = unsafe { l2_table_ptr.as_mut() };
    let l2_entry = &mut (l2_table.entries[i2]);

    if !l2_entry.is_valid() {
        return;
    }

    l2_entry.set_is_valid(false);
    l2_entry.set_address(0);

    if is_table_empty(l2_table) {
        allocator.free_table(l2_table_ptr);
        l1_entry.set_is_valid(false);
        l1_entry.set_address(0);

        if is_table_empty(l1_table) {
            allocator.free_table(l1_table_ptr);
            l0_entry.set_is_valid(false);
            l0_entry.set_address(0);
        }
    }
}

pub fn map_page<A: TableAllocator>(
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
        let l1_va = allocator.phys_to_virt(l1_pa);
        let mut z = unsafe { NonNull::new_unchecked(l1_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(dmap_addr_to_phys(table.as_ptr() as u64));

        unsafe { table.as_mut() }
    };

    let l1_entry = &mut (l1_table.entries[i1]);

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        let l2_va = allocator.phys_to_virt(l2_pa);
        let mut z = unsafe { NonNull::new_unchecked(l2_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(allocator.virt_to_phys(table.as_ptr()));

        unsafe { table.as_mut() }
    };

    let l2_entry = &mut (l2_table.entries[i2]);

    let l3_table = if l2_entry.is_valid() {
        let l3_pa = l2_entry.address();
        let l3_va = allocator.phys_to_virt(l3_pa);
        let mut table: NonNull<TTable<TABLE_ENTRIES>> =
            unsafe { NonNull::new_unchecked(l3_va as *mut _) };

        unsafe { table.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l2_entry.set_is_valid(true);
        l2_entry.set_is_table();
        l2_entry.set_address(allocator.virt_to_phys(table.as_ptr()));

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

pub fn unmap_page<A: TableAllocator>(root: &mut TTable<TABLE_ENTRIES>, va: usize, allocator: &A) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);
    let i3 = TTENATIVE::calculate_index(va as u64, 3);

    let l0_entry = &mut (root.entries[i0]);
    if !l0_entry.is_valid() || !l0_entry.is_table() {
        return;
    }

    let l1_pa = l0_entry.address();
    let l1_va = allocator.phys_to_virt(l1_pa);
    let mut l1_table_ptr = unsafe { NonNull::new_unchecked(l1_va as *mut TTable<TABLE_ENTRIES>) };
    let l1_table = unsafe { l1_table_ptr.as_mut() };
    let l1_entry = &mut (l1_table.entries[i1]);

    if !l1_entry.is_valid() || !l1_entry.is_table() {
        return;
    }

    let l2_pa = l1_entry.address();
    let l2_va = allocator.phys_to_virt(l2_pa);
    let mut l2_table_ptr = unsafe { NonNull::new_unchecked(l2_va as *mut TTable<TABLE_ENTRIES>) };
    let l2_table = unsafe { l2_table_ptr.as_mut() };
    let l2_entry = &mut (l2_table.entries[i2]);

    if !l2_entry.is_valid() || !l2_entry.is_table() {
        return;
    }

    let l3_pa = l2_entry.address();
    let l3_va = allocator.phys_to_virt(l3_pa);
    let mut l3_table_ptr = unsafe { NonNull::new_unchecked(l3_va as *mut TTable<TABLE_ENTRIES>) };
    let l3_table = unsafe { l3_table_ptr.as_mut() };
    let l3_entry = &mut (l3_table.entries[i3]);

    if !l3_entry.is_valid() {
        return;
    }

    l3_entry.set_is_valid(false);
    l3_entry.set_address(0);

    if is_table_empty(l3_table) {
        allocator.free_table(l3_table_ptr);
        l2_entry.set_is_valid(false);
        l2_entry.set_address(0);

        if is_table_empty(l2_table) {
            allocator.free_table(l2_table_ptr);
            l1_entry.set_is_valid(false);
            l1_entry.set_address(0);

            if is_table_empty(l1_table) {
                allocator.free_table(l1_table_ptr);
                l0_entry.set_is_valid(false);
                l0_entry.set_address(0);
            }
        }
    }
}

pub fn free_tables<A: TableAllocator>(mut root: NonNull<TTable<TABLE_ENTRIES>>, allocator: &A) {
    let root_table = unsafe { root.as_mut() };
    for i0 in 0..2 {
        let l0_entry = &mut (root_table.entries[i0]);

        if !l0_entry.is_valid() || !l0_entry.is_table() {
            continue;
        }

        let l1_pa = l0_entry.address();
        let l1_va = phys_addr_to_dmap(l1_pa);

        let mut l1_table_ptr =
            unsafe { NonNull::new_unchecked(l1_va as *mut TTable<TABLE_ENTRIES>) };
        let l1_table = unsafe { l1_table_ptr.as_mut() };

        for i1 in 0..TABLE_ENTRIES {
            let l1_entry = &mut (l1_table.entries[i1]);

            if !l1_entry.is_valid() || !l1_entry.is_table() {
                continue;
            }

            let l2_pa = l1_entry.address();
            let l2_va = phys_addr_to_dmap(l2_pa);

            let mut l2_table_ptr =
                unsafe { NonNull::new_unchecked(l2_va as *mut TTable<TABLE_ENTRIES>) };
            let l2_table = unsafe { l2_table_ptr.as_mut() };

            for i2 in 0..TABLE_ENTRIES {
                let l2_entry = &mut (l2_table.entries[i2]);

                if !l2_entry.is_valid() || !l2_entry.is_table() {
                    continue;
                }

                let l3_pa = l2_entry.address();
                let l3_va = phys_addr_to_dmap(l3_pa);

                let mut l3_table_ptr =
                    unsafe { NonNull::new_unchecked(l3_va as *mut TTable<TABLE_ENTRIES>) };
                let l3_table = unsafe { l3_table_ptr.as_mut() };

                for i3 in 0..TABLE_ENTRIES {
                    let l3_entry = &mut (l3_table.entries[i3]);

                    if !l3_entry.is_valid() || !l3_entry.is_table() {
                        continue;
                    }

                    l3_entry.set_is_valid(false);
                }

                allocator.free_table(unsafe { NonNull::new_unchecked(l3_table) });
            }
            allocator.free_table(unsafe { NonNull::new_unchecked(l2_table) });
        }
        allocator.free_table(unsafe { NonNull::new_unchecked(l1_table) });
    }
    allocator.free_table(unsafe { NonNull::new_unchecked(root_table) });
}

fn is_table_empty(table: &TTable<TABLE_ENTRIES>) -> bool {
    for i in 0..TABLE_ENTRIES {
        let entry = &table.entries[i];
        if entry.is_valid() {
            return false;
        }
    }
    true
}
