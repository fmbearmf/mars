use core::ptr::NonNull;

use aarch64_cpu::registers::{Readable, TTBR0_EL1};
use klib::vm::{PAGE_SIZE, TABLE_ENTRIES, TTENATIVE, TTable, TTableUEFI, map::TableAllocator};
use uefi::boot::{self, MemoryType, PAGE_SIZE as UEFI_PS};

pub struct UefiPTAllocator;

impl UefiPTAllocator {
    pub const fn new() -> Self {
        Self {}
    }

    pub fn vaddr_to_paddr_uefi(&self, vaddr: usize) -> usize {
        type TTable4K = TTableUEFI;
        let table_addr = TTBR0_EL1.get_baddr() as *const TTable4K;

        let i0 = (vaddr >> 39) & 0x1FF;
        let i1 = (vaddr >> 30) & 0x1FF;
        let i2 = (vaddr >> 21) & 0x1FF;
        let i3 = (vaddr >> 12) & 0x1FF;
        let mut offset = vaddr & 0xFFF;

        let l1_table_addr = (&unsafe { *table_addr }).entries[i0].address();
        if (l1_table_addr as *const TTable4K).is_null() {
            panic!("l1 table l0[{}] null: {}", i0, l1_table_addr);
        }

        let l1_table = l1_table_addr as *const TTable4K;
        let l1_entry = (&unsafe { *l1_table }).entries[i1];

        if l1_entry.is_block() {
            offset = vaddr & ((1usize << 30) - 1);
            return l1_entry.address() as usize + offset;
        }

        let l2_table_addr = (l1_entry.address()) as *const TTable4K;
        if l2_table_addr.is_null() {
            panic!("l2 table l1[{}] null: {}", i1, l2_table_addr as u64);
        }

        let l2_table = l2_table_addr as *const TTable4K;
        let l2_entry = (&unsafe { *l2_table }).entries[i2];

        if l2_entry.is_block() {
            offset = vaddr & ((1usize << 21) - 1);
            return l2_entry.address() as usize + offset;
        }

        let l3_table_addr = (l2_entry.address()) as *const TTable4K;
        if l3_table_addr.is_null() {
            panic!("l3 table l2[{}] null: {}", i2, l3_table_addr as u64);
        }

        let l3_table = l3_table_addr as *const TTable4K;
        let l3_entry = (&unsafe { *l3_table }).entries[i3];

        l3_entry.address() as usize + offset
    }
}

impl TableAllocator for UefiPTAllocator {
    fn alloc_table(&self) -> NonNull<TTable<TABLE_ENTRIES>> {
        const SIZE: usize = size_of::<TTable<TABLE_ENTRIES>>() + PAGE_SIZE;

        let table_result = boot::allocate_pages(
            boot::AllocateType::AnyPages,
            MemoryType::LOADER_DATA,
            SIZE / UEFI_PS,
        );

        let table: NonNull<TTable<TABLE_ENTRIES>> = table_result.unwrap().cast();

        let addr = table.as_ptr() as usize;
        let aligned = TTENATIVE::align_up(addr as u64);

        let mut table_aligned = NonNull::new(aligned as *mut TTable<TABLE_ENTRIES>).unwrap();

        let table_aligned_mut = unsafe { table_aligned.as_mut() };
        *table_aligned_mut = TTable::new();

        table_aligned
    }
}
