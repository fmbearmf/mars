use core::ptr::NonNull;

use aarch64_cpu::asm::barrier::{self, dsb, isb};
use aarch64_cpu_ext::{
    asm::cache::{Iallu, ic},
    structures::tte::{AccessPermission, Shareability},
};

use crate::vm::{
    L0_BLOCK_SIZE, L1_BLOCK_SIZE, L2_BLOCK_SIZE, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE, TABLE_ENTRIES,
    TTENATIVE, TTable,
};

pub trait TableAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>>;
    fn free_table(&self, table: NonNull<TTable<TABLE_ENTRIES>>);
}

pub trait AddressTranslator {
    fn phys_to_dmap<T>(phys: u64) -> *mut T;
    fn dmap_to_phys<T>(virt: *mut T) -> u64;
}

pub fn map_region<T: TableAllocator, A: AddressTranslator>(
    root: &mut TTable<TABLE_ENTRIES>,
    pa: usize,
    va: usize,
    size: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &T,
) {
    assert_eq!(va & PAGE_MASK, 0, "VA must be page aligned");
    assert_eq!(pa & PAGE_MASK, 0, "PA must be page aligned");
    assert_eq!(size & PAGE_MASK, 0, "size must be page aligned");
    assert!(size >= PAGE_SIZE, "can't map less than 1 page!");

    let num_pages = size >> PAGE_SHIFT;

    for i in 0..num_pages {
        let vaddr = va + (i << PAGE_SHIFT);
        let paddr = pa + (i << PAGE_SHIFT);
        map_page::<T, A>(
            root, paddr, vaddr, access, share, uxn, pxn, attr_index, allocator,
        );
    }
}

pub fn unmap_region<T: TableAllocator, A: AddressTranslator>(
    root: &mut TTable<TABLE_ENTRIES>,
    va: usize,
    size: usize,
    allocator: &T,
) {
    assert_eq!(va & PAGE_MASK, 0, "address must be page-aligned");
    assert_eq!(size & PAGE_MASK, 0, "size must be page-aligned");

    let num_pages = size >> PAGE_SHIFT;

    for i in 0..num_pages {
        let vaddr = va + (i << PAGE_SHIFT);
        unmap_page::<T, A>(root, vaddr, allocator);
    }
}

pub fn id_map<T: TableAllocator, A: AddressTranslator>(
    root: &mut TTable<TABLE_ENTRIES>,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &T,
) {
    const BLOCKS_NEEDED: usize = L1_BLOCK_SIZE / L2_BLOCK_SIZE;

    for i in 0..BLOCKS_NEEDED {
        let current_addr = i * L2_BLOCK_SIZE;

        map_l2_block::<_, A>(
            root,
            current_addr,
            current_addr,
            access,
            share,
            uxn,
            pxn,
            attr_index,
            allocator,
        );
    }
}

pub fn clone_page_tables<T: TableAllocator, A: AddressTranslator>(
    old: &TTable<TABLE_ENTRIES>,
    allocator: &T,
) -> NonNull<TTable<TABLE_ENTRIES>> {
    clone_l0::<T, A>(old, allocator)
}

macro_rules! impl_clone_pt_level {
    ($name:ident, $next: ident) => {
        fn $name<T: TableAllocator, A: AddressTranslator>(
            src: &TTable<TABLE_ENTRIES>,
            allocator: &T,
        ) -> NonNull<TTable<TABLE_ENTRIES>> {
            let mut new_table_ptr = allocator.alloc_table();
            let new_table = unsafe { new_table_ptr.as_mut() };

            for i in 0..TABLE_ENTRIES {
                let entry = &src.entries[i];
                if !entry.is_valid() {
                    continue;
                }

                if entry.is_table() {
                    let child_src = unsafe { &*(entry.address() as *const TTable<TABLE_ENTRIES>) };
                    let child_dst = $next::<T, A>(child_src, allocator);

                    let mut new_entry = *entry;
                    new_entry.set_address(child_dst.as_ptr() as _);
                    new_table.entries[i] = new_entry;
                } else {
                    new_table.entries[i] = *entry;
                }
            }

            new_table_ptr
        }
    };
}

// base case
fn clone_l3<T: TableAllocator, A: AddressTranslator>(
    src: &TTable<TABLE_ENTRIES>,
    allocator: &T,
) -> NonNull<TTable<TABLE_ENTRIES>> {
    let mut new_table_ptr = allocator.alloc_table();
    let new_table = unsafe { new_table_ptr.as_mut() };

    for i in 0..TABLE_ENTRIES {
        new_table.entries[i] = src.entries[i];
    }

    new_table_ptr
}

impl_clone_pt_level!(clone_l2, clone_l3);
impl_clone_pt_level!(clone_l1, clone_l2);
impl_clone_pt_level!(clone_l0, clone_l1);

pub fn map_l2_block<T: TableAllocator, A: AddressTranslator>(
    root: &mut TTable<TABLE_ENTRIES>,
    pa: usize,
    va: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &T,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);

    let l0_entry = &mut (root.entries[i0]);

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        let l1_va = A::phys_to_dmap(l1_pa);
        let mut z = unsafe { NonNull::new_unchecked(l1_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(A::dmap_to_phys(table.as_ptr()));

        unsafe { table.as_mut() }
    };

    let l1_entry = &mut (l1_table.entries[i1]);

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        let l2_va = A::phys_to_dmap(l2_pa);
        let mut z = unsafe { NonNull::new_unchecked(l2_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(A::dmap_to_phys(table.as_ptr()));

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

pub fn map_page<T: TableAllocator, A: AddressTranslator>(
    root: &mut TTable<TABLE_ENTRIES>,
    pa: usize,
    va: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
    allocator: &T,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);
    let i3 = TTENATIVE::calculate_index(va as u64, 3);

    let l0_entry = &mut (root.entries[i0]);

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        let l1_va = A::phys_to_dmap(l1_pa);
        let mut z = unsafe { NonNull::new_unchecked(l1_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(A::dmap_to_phys(table.as_ptr()));

        unsafe { table.as_mut() }
    };

    let l1_entry = &mut (l1_table.entries[i1]);

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        let l2_va = A::phys_to_dmap(l2_pa);
        let mut z = unsafe { NonNull::new_unchecked(l2_va as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(A::dmap_to_phys(table.as_ptr()));

        unsafe { table.as_mut() }
    };

    let l2_entry = &mut (l2_table.entries[i2]);

    let l3_table = if l2_entry.is_valid() {
        let l3_pa = l2_entry.address();
        let l3_va = A::phys_to_dmap(l3_pa);
        let mut table: NonNull<TTable<TABLE_ENTRIES>> =
            unsafe { NonNull::new_unchecked(l3_va as *mut _) };

        unsafe { table.as_mut() }
    } else {
        let mut table = allocator.alloc_table();

        l2_entry.set_is_valid(true);
        l2_entry.set_is_table();
        l2_entry.set_address(A::dmap_to_phys(table.as_ptr()));

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

pub fn unmap_page<T: TableAllocator, A: AddressTranslator>(
    root: &mut TTable<TABLE_ENTRIES>,
    va: usize,
    allocator: &T,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);
    let i3 = TTENATIVE::calculate_index(va as u64, 3);

    let l0_entry = &mut (root.entries[i0]);
    if !l0_entry.is_valid() || !l0_entry.is_table() {
        return;
    }

    let l1_pa = l0_entry.address();
    let l1_va = A::phys_to_dmap(l1_pa);
    let mut l1_table_ptr = unsafe { NonNull::new_unchecked(l1_va as *mut TTable<TABLE_ENTRIES>) };
    let l1_table = unsafe { l1_table_ptr.as_mut() };
    let l1_entry = &mut (l1_table.entries[i1]);

    if !l1_entry.is_valid() || !l1_entry.is_table() {
        return;
    }

    let l2_pa = l1_entry.address();
    let l2_va = A::phys_to_dmap(l2_pa);
    let mut l2_table_ptr = unsafe { NonNull::new_unchecked(l2_va as *mut TTable<TABLE_ENTRIES>) };
    let l2_table = unsafe { l2_table_ptr.as_mut() };
    let l2_entry = &mut (l2_table.entries[i2]);

    if !l2_entry.is_valid() || !l2_entry.is_table() {
        return;
    }

    let l3_pa = l2_entry.address();
    let l3_va = A::phys_to_dmap(l3_pa);
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

pub fn free_tables<T: TableAllocator, A: AddressTranslator>(
    mut root: NonNull<TTable<TABLE_ENTRIES>>,
    allocator: &T,
) {
    let root_table = unsafe { root.as_mut() };
    for i0 in 0..2 {
        let l0_entry = &mut (root_table.entries[i0]);

        if !l0_entry.is_valid() || !l0_entry.is_table() {
            continue;
        }

        let l1_pa = l0_entry.address();
        let l1_va = A::phys_to_dmap(l1_pa);

        let mut l1_table_ptr =
            unsafe { NonNull::new_unchecked(l1_va as *mut TTable<TABLE_ENTRIES>) };
        let l1_table = unsafe { l1_table_ptr.as_mut() };

        for i1 in 0..TABLE_ENTRIES {
            let l1_entry = &mut (l1_table.entries[i1]);

            if !l1_entry.is_valid() || !l1_entry.is_table() {
                continue;
            }

            let l2_pa = l1_entry.address();
            let l2_va = A::phys_to_dmap(l2_pa);

            let mut l2_table_ptr =
                unsafe { NonNull::new_unchecked(l2_va as *mut TTable<TABLE_ENTRIES>) };
            let l2_table = unsafe { l2_table_ptr.as_mut() };

            for i2 in 0..TABLE_ENTRIES {
                let l2_entry = &mut (l2_table.entries[i2]);

                if !l2_entry.is_valid() || !l2_entry.is_table() {
                    continue;
                }

                let l3_pa = l2_entry.address();
                let l3_va = A::phys_to_dmap(l3_pa);

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
