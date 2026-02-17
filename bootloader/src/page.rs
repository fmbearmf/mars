use core::ptr::{NonNull, read_volatile, write_volatile};

use aarch64_cpu::{
    asm::{
        barrier::{self, dsb, isb},
        nop,
    },
    registers::{CPACR_EL1, MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use alloc::boxed::Box;
use klib::vm::{
    L2_BLOCK_MASK, L2_BLOCK_SHIFT, MAIR_DEVICE_INDEX, PAGE_MASK, PAGE_SHIFT, PAGE_SIZE,
    TABLE_ENTRIES, TTENATIVE, TTable, TTableUEFI,
};
use tock_registers::interfaces::*;
use uefi::boot::{self, AllocateType, MemoryType, PAGE_SIZE as UEFI_PS};

pub fn cpu_init() {
    MAIR_EL1.modify(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);
    dsb(barrier::SY);
}

pub fn mmu_init(root_pt: *const TTable<TABLE_ENTRIES>) {
    MAIR_EL1.modify(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    TCR_EL1.write(
        TCR_EL1::TBI1::Ignored
            + TCR_EL1::IPS::Bits_48
            + TCR_EL1::TG1::KiB_16
            + TCR_EL1::SH1::Inner
            + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD1::EnableTTBR1Walks
            + TCR_EL1::T1SZ.val(16)
            + TCR_EL1::TBI0::Ignored
            + TCR_EL1::TG0::KiB_4
            + TCR_EL1::SH1::Inner
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD0::EnableTTBR0Walks
            + TCR_EL1::T0SZ.val(16),
    );

    TTBR1_EL1.set(root_pt as u64);
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    isb(barrier::SY);
    dsb(barrier::SY);
}

pub fn map_region(
    root: &mut TTable<TABLE_ENTRIES>,
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
        map_page(root, paddr, vaddr, access, share, uxn, pxn, attr_index);
    }
    //}
}

fn map_l2_block(
    root: &mut TTable<TABLE_ENTRIES>,
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

    let l0_entry = &mut (root.entries[i0]);

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        let mut z = unsafe { NonNull::new_unchecked(l1_pa as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = alloc_table();

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
        let mut table = alloc_table();

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
fn map_page(
    root: &mut TTable<TABLE_ENTRIES>,
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

    //info!("i0: {} i1: {} i2: {} i3: {}", i0, i1, i2, i3);

    let l0_entry = &mut (root.entries[i0]);
    //info!("l0_entry addr: {:#x}", l0_entry as *const _ as u64);

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        let mut z = unsafe { NonNull::new_unchecked(l1_pa as *mut _) };
        unsafe { z.as_mut() }
    } else {
        let mut table = alloc_table();

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
        let mut table = alloc_table();

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
        let mut table = alloc_table();

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

pub fn alloc_table() -> NonNull<TTable<TABLE_ENTRIES>> {
    const SIZE: usize = size_of::<TTable<TABLE_ENTRIES>>() + PAGE_SIZE;

    let table_result = boot::allocate_pages(
        AllocateType::AnyPages,
        MemoryType::LOADER_DATA,
        SIZE / UEFI_PS,
    );

    let table: NonNull<TTable<TABLE_ENTRIES>> = table_result.unwrap().cast();

    let addr = table.as_ptr() as usize;
    let aligned = TTENATIVE::align_up(addr as u64);

    let mut table_aligned =
        unsafe { NonNull::new_unchecked(aligned as *mut TTable<TABLE_ENTRIES>) };

    let table_aligned_mut = unsafe { table_aligned.as_mut() };

    *table_aligned_mut = TTable::new();

    //info!("ALLOC table @ {:#x} of size {}", aligned, real_size);

    table_aligned
}

pub fn uefi_addr_to_paddr(vaddr: usize) -> usize {
    type TTable4K = TTableUEFI;
    // DO NOT make this a reference.
    // rust is fucking STUPID and reserves stack space for the entire table and causes an overflow.
    let table_addr = TTBR0_EL1.get() as *const TTable4K;

    let i0 = (vaddr >> 39) & 0x1FF;
    let i1 = (vaddr >> 30) & 0x1FF;
    let i2 = (vaddr >> 21) & 0x1FF;
    let i3 = (vaddr >> 12) & 0x1FF;
    let mut offset = vaddr & 0xFFF;

    let l1_table_addr = (&unsafe { *table_addr }).entries[i0].address();
    if (l1_table_addr as *const TTable4K).is_null() {
        panic!("l1 table (l0[{}]) null: {}", i0, l1_table_addr as u64);
    }

    let l1_table = l1_table_addr as *const TTable4K;
    let l1_entry = (&unsafe { *l1_table }).entries[i1];

    if l1_entry.is_block() {
        offset = vaddr & ((1usize << 30) - 1);
        //info!(
        //    "L1 {:#x} block {} entry: {:#x}",
        //    l1_table_addr as u64,
        //    i1,
        //    l1_entry.get()
        //);
        return l1_entry.address() as usize + offset;
    }

    let l2_table_addr = (l1_entry.address()) as *const TTable4K;

    if l2_table_addr.is_null() {
        panic!("l2 table (l1[{}]) null: {}", i1, l2_table_addr as u64);
    }
    let l2_table = l2_table_addr as *const TTable4K;
    let l2_entry = (&unsafe { *l2_table }).entries[i2];

    if l2_entry.is_block() {
        offset = vaddr & ((1usize << 21) - 1);
        //info!(
        //    "L2 block entry @ {:#x} = {:#x} (offset {:#x})",
        //    l2_table_addr as usize,
        //    l2_entry.address(),
        //    offset
        //);
        return l2_entry.address() as usize + offset;
    }

    let l3_table_addr = (l2_entry.address()) as *const TTable4K;

    if l3_table_addr.is_null() {
        panic!("l3 table (l2[{}]) null: {}", i2, l3_table_addr as u64);
    }

    let l3_table = l3_table_addr as *const TTable4K;
    let l3_entry = (&unsafe { *l3_table }).entries[i3];

    if l3_entry.is_table() {
        //info!(
        //    "L3 PTE entry @ {:#x} = {:#x} (offset {:#x})",
        //    l3_table_addr as usize,
        //    l3_entry.address(),
        //    offset
        //);
    }

    l3_entry.address() as usize + offset
}
