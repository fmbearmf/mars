use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::MAIR_EL1,
};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::boxed::Box;
use log::info;
use mars_klib::vm::{
    L2_BLOCK_MASK, L2_BLOCK_SHIFT, MAIR_DEVICE_INDEX, PAGE_MASK, PAGE_SHIFT, TABLE_ENTRIES,
    TTENATIVE, TTable,
};
use tock_registers::interfaces::*;

pub fn mmu_init() {
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );
    isb(barrier::SY);
    dsb(barrier::SY);
}

pub fn map_region<const N: usize>(
    root: &mut TTable<N>,
    pa: usize,
    va: usize,
    size: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
) {
    if va & PAGE_MASK != 0 || pa & PAGE_MASK != 0 || size & PAGE_MASK != 0 {
        panic!("addresses AND size must be page aligned");
    }

    if va & L2_BLOCK_MASK == 0 && pa & L2_BLOCK_MASK == 0 && size & L2_BLOCK_MASK == 0 {
        let num_blocks = size >> L2_BLOCK_SHIFT;

        for i in 0..num_blocks {
            let vaddr = va + (i << L2_BLOCK_SHIFT);
            let paddr = pa + (i << L2_BLOCK_SHIFT);
            map_l2_block(root, paddr, vaddr, access, share, uxn, pxn, attr_index);
        }

        return;
    } else {
        let num_pages = size >> PAGE_SHIFT;

        for i in 0..num_pages {
            let vaddr = va + (i << PAGE_SHIFT);
            let paddr = pa + (i << PAGE_SHIFT);
            map_page(root, paddr, vaddr, access, share, uxn, pxn, attr_index);
        }
    }
}

fn map_l2_block<const N: usize>(
    root: &TTable<N>,
    pa: usize,
    va: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);

    let mut l0_entry = root.entries[i0];

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        l1_pa as *mut TTable<N>
    } else {
        let mut new_table: Box<TTable<N>> = Box::new(TTable::new());
        let new_pa = &*new_table;

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(new_pa as *const _ as u64);

        &raw mut *new_table
    };

    let mut l1_entry = unsafe { *l1_table }.entries[i1];

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        l2_pa as *mut TTable<N>
    } else {
        let mut new_table: Box<TTable<N>> = Box::new(TTable::new());
        info!(
            "ALLOC box @ {:#x} of size {}",
            &raw const *new_table as *const _ as u64,
            size_of::<TTable<N>>()
        );
        let new_pa = &*new_table;

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(new_pa as *const _ as u64);

        &raw mut *new_table
    };

    let mut l2_entry = unsafe { *l2_table }.entries[i2];

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

fn map_page<const N: usize>(
    root: &TTable<N>,
    pa: usize,
    va: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
) {
    let i0 = TTENATIVE::calculate_index(va as u64, 0);
    let i1 = TTENATIVE::calculate_index(va as u64, 1);
    let i2 = TTENATIVE::calculate_index(va as u64, 2);
    let i3 = TTENATIVE::calculate_index(va as u64, 3);

    let mut l0_entry = root.entries[i0];

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        l1_pa as *mut TTable<N>
    } else {
        let mut new_table: Box<TTable<N>> = Box::new(TTable::new());
        let new_pa = &*new_table;

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(new_pa as *const _ as u64);

        &raw mut *new_table
    };

    let mut l1_entry = unsafe { *l1_table }.entries[i1];

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        l2_pa as *mut TTable<N>
    } else {
        let mut new_table: Box<TTable<N>> = Box::new(TTable::new());
        let new_pa = &*new_table;

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(new_pa as *const _ as u64);

        &raw mut *new_table
    };

    let mut l2_entry = unsafe { *l2_table }.entries[i2];

    let l3_table = if l2_entry.is_valid() {
        let l3_pa = l2_entry.address();
        l3_pa as *mut TTable<N>
    } else {
        let mut new_table: Box<TTable<N>> = Box::new(TTable::new());
        let new_pa = &*new_table;

        l2_entry.set_is_valid(true);
        l2_entry.set_is_table();
        l2_entry.set_address(new_pa as *const _ as u64);

        &raw mut *new_table
    };

    let mut l3_entry = unsafe { *l3_table }.entries[i3];

    l3_entry.set_is_valid(true);
    l3_entry.set_is_block();
    l3_entry.set_address(pa as u64);
    l3_entry.set_access();
    l3_entry.set_access_permission(access);
    l3_entry.set_shareability(share);
    l3_entry.set_attr_index(attr_index);
    l3_entry.set_executable(!uxn);
    l3_entry.set_privileged_executable(!pxn);
}
