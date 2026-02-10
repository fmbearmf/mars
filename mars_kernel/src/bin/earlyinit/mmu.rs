use aarch64_cpu::{
    asm::{self, nop},
    registers::{MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use core::mem::transmute;
use mars_klib::vm::{
    DMAP_START, L2_BLOCK_MASK, L2_BLOCK_SHIFT, L2_BLOCK_SIZE, MAIR_DEVICE_INDEX, MAIR_NORMAL_INDEX,
    PAGE_MASK, PAGE_SHIFT, TABLE_ENTRIES, TTENATIVE, TTable, align_down, align_up,
};
use tock_registers::interfaces::*;
use uefi::{
    boot::{MemoryType, PAGE_SIZE as UEFI_PAGE_SIZE},
    mem::memory_map::{MemoryMap, MemoryMapIter, MemoryMapOwned, MemoryMapRefMut},
};

use crate::{
    busy_loop_ret,
    earlyinit::earlymem::{MAX_TABLES, alloc_table},
};

unsafe extern "C" {
    static __KEND: usize;
}

#[unsafe(link_section = ".reclaimable.bss")]
static mut PT_POOL: [TTable<TABLE_ENTRIES>; MAX_TABLES] = [TTable::new(); MAX_TABLES];

pub extern "C" fn init_mmu(load_addr: usize, offset: usize, mmap: &mut MemoryMapOwned) -> ! {
    // attr0=device, attr1=normal
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    let (LOW_L0, HIGH_L0) = unsafe { setup_tables(load_addr, offset, mmap, DMAP_START) };
    busy_loop_ret();

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
            + TCR_EL1::TG0::KiB_16
            + TCR_EL1::SH0::Inner
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD0::EnableTTBR0Walks
            + TCR_EL1::T0SZ.val(16),
    );

    TTBR0_EL1.set(LOW_L0 as *const _ as u64);
    TTBR1_EL1.set(HIGH_L0 as *const _ as u64);
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    asm::barrier::isb(asm::barrier::SY);
    asm::barrier::dsb(asm::barrier::SY);
    asm::barrier::dsb(asm::barrier::ISH);

    let phys_entry = crate::arm_init as *const () as usize;
    let init_virt: fn() -> ! = unsafe { transmute(phys_entry) };

    nop();
    init_virt()
}

unsafe fn setup_tables(
    load_addr: usize,
    offset: usize,
    mmap: &mut MemoryMapOwned,
    dmap_base: usize,
) -> (
    &'static TTable<TABLE_ENTRIES>,
    &'static TTable<TABLE_ENTRIES>,
) {
    let pool: *const [TTable<TABLE_ENTRIES>] = unsafe { &raw const PT_POOL };

    let root0 = alloc_table(pool).expect("no free space for root0");
    let root1 = alloc_table(pool).expect("no free space for root1");

    let kernel_vma_start = load_addr + offset;
    let kernel_vma_end = unsafe { &__KEND as *const _ as usize };
    let kernel_vma_size = kernel_vma_end - kernel_vma_start;

    let phys_kernel_start = TTENATIVE::align_down(load_addr as u64);
    let phys_kernel_end = TTENATIVE::align_up((load_addr + kernel_vma_size) as u64);
    let kernel_size = phys_kernel_end - phys_kernel_start;

    let kernel_high_start = TTENATIVE::align_down(kernel_vma_start as u64);
    let kernel_high_end = TTENATIVE::align_up((kernel_vma_start + kernel_vma_size) as u64);

    unsafe {
        map_region(
            pool,
            root0,
            align_down(phys_kernel_start as usize, L2_BLOCK_SIZE),
            align_down(phys_kernel_start as usize, L2_BLOCK_SIZE),
            align_up(kernel_size as usize, L2_BLOCK_SIZE),
            AccessPermission::PrivilegedReadWrite,
            Shareability::InnerShareable,
            true,
            false,
            MAIR_NORMAL_INDEX,
        )
    };

    unsafe {
        map_region(
            pool,
            root1,
            align_down(phys_kernel_start as usize, L2_BLOCK_SIZE),
            align_down(kernel_high_start as usize, L2_BLOCK_SIZE),
            align_up(kernel_size as usize, L2_BLOCK_SIZE),
            AccessPermission::PrivilegedReadWrite,
            Shareability::InnerShareable,
            true,
            false,
            MAIR_NORMAL_INDEX,
        )
    };
    busy_loop_ret();

    let mmap_len = mmap.len();

    let mut max_uefi_pa = 0usize;
    busy_loop_ret();
    for i in 0..mmap_len {
        busy_loop_ret();
        let d = mmap.get(i).expect("mmap entry missing");
        let pa = d.phys_start as usize;
        let pages = d.page_count as usize;
        if pages == 0 {
            continue;
        }

        let size = pages * UEFI_PAGE_SIZE;
        let end = pa.checked_add(size - 1).expect("mmap overflow");
        if end > max_uefi_pa {
            max_uefi_pa = end;
        }
    }
    //let dmap_size = TTENATIVE::align_up(max_uefi_pa.checked_add(1).unwrap_or(0) as u64);

    for i in 0..mmap_len {
        let d = mmap.get(i).expect("mmap entry missing");
        busy_loop_ret();
        let pa = TTENATIVE::align_down(d.phys_start) as usize;

        let pages = d.page_count;
        if pages == 0 {
            continue;
        }

        let mut region_size = pages * UEFI_PAGE_SIZE as u64;

        region_size =
            TTENATIVE::align_up(region_size + (d.phys_start.checked_sub(pa as u64).unwrap()));

        let mut va = pa.checked_add(dmap_base).expect("dmap VA overflow");
        va = TTENATIVE::align_down(va as u64) as usize;

        match d.ty {
            MemoryType::CONVENTIONAL
            | MemoryType::LOADER_DATA
            | MemoryType::BOOT_SERVICES_DATA
            | MemoryType::RUNTIME_SERVICES_DATA
            | MemoryType::ACPI_RECLAIM => {
                unsafe {
                    map_region(
                        pool,
                        root1,
                        pa,
                        va,
                        region_size as usize,
                        AccessPermission::PrivilegedReadWrite,
                        Shareability::InnerShareable,
                        true,
                        true,
                        MAIR_NORMAL_INDEX,
                    )
                };
            }

            MemoryType::LOADER_CODE
            | MemoryType::BOOT_SERVICES_CODE
            | MemoryType::RUNTIME_SERVICES_CODE => {
                let exec = matches!(d.ty, MemoryType::RUNTIME_SERVICES_CODE);
                unsafe {
                    map_region(
                        pool,
                        root1,
                        pa,
                        va,
                        region_size as usize,
                        AccessPermission::PrivilegedReadWrite,
                        Shareability::InnerShareable,
                        true,
                        !exec,
                        MAIR_NORMAL_INDEX,
                    )
                };
            }

            MemoryType::MMIO | MemoryType::MMIO_PORT_SPACE | MemoryType::ACPI_NON_VOLATILE => {
                unsafe {
                    map_region(
                        pool,
                        root1,
                        pa,
                        va,
                        region_size as usize,
                        AccessPermission::PrivilegedReadWrite,
                        Shareability::OuterShareable,
                        true,
                        true,
                        MAIR_DEVICE_INDEX,
                    )
                };
            }

            MemoryType::RESERVED
            | MemoryType::UNUSABLE
            | MemoryType::PAL_CODE
            | MemoryType::MAX => {}

            _ => {
                unsafe {
                    map_region(
                        pool,
                        root1,
                        pa,
                        va,
                        region_size as usize,
                        AccessPermission::PrivilegedReadWrite,
                        Shareability::InnerShareable,
                        true,
                        true,
                        MAIR_DEVICE_INDEX,
                    )
                };
            }
        }
    }

    busy_loop_ret();

    unsafe { (&*root0, &*root1) }
}

#[inline]
fn make_block(
    pa: usize,
    access: AccessPermission,
    share: Shareability,
    uxn: bool,
    pxn: bool,
    attr_index: u64,
) -> TTENATIVE {
    let mut tte = TTENATIVE::new_block(pa as u64);
    tte.set_attr_index(attr_index);
    tte.set_access_permission(access);
    tte.set_shareability(share);
    tte.set_executable(!uxn);
    tte.set_privileged_executable(!pxn);

    tte
}

unsafe fn map_region<const N: usize>(
    pool: *const [TTable<N>],
    root: *mut TTable<N>,
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
            map_l2_block(
                pool, root, paddr, vaddr, access, share, uxn, pxn, attr_index,
            );
        }

        return;
    } else {
        let num_pages = size >> PAGE_SHIFT;

        for i in 0..num_pages {
            let vaddr = va + (i << PAGE_SHIFT);
            let paddr = pa + (i << PAGE_SHIFT);
            map_page(
                pool, root, paddr, vaddr, access, share, uxn, pxn, attr_index,
            );
        }
    }
}

fn map_l2_block<const N: usize>(
    pool: *const [TTable<N>],
    root: *mut TTable<N>,
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

    let mut l0_entry = unsafe { *root }.entries[i0];

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        l1_pa as *mut TTable<N>
    } else {
        let new_table = alloc_table(pool).expect("pt_pool exhausted");
        let new_pa = new_table as *const _ as u64;

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(new_pa);

        new_table
    };

    let mut l1_entry = unsafe { *l1_table }.entries[i1];

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        l2_pa as *mut TTable<N>
    } else {
        busy_loop_ret();
        let new_table = alloc_table(pool).expect("pt_pool exhausted");
        let new_pa = new_table as *const _ as u64;

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(new_pa);

        new_table
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
    pool: *const [TTable<N>],
    root: *mut TTable<N>,
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

    let mut l0_entry = unsafe { *root }.entries[i0];

    let l1_table = if l0_entry.address() != 0 && l0_entry.is_table() {
        let l1_pa = l0_entry.address();
        l1_pa as *mut TTable<N>
    } else {
        let new_table = alloc_table(pool).expect("pt_pool exhausted");
        let new_pa = new_table as *const _ as u64;

        l0_entry.set_is_valid(true);
        l0_entry.set_is_table();
        l0_entry.set_address(new_pa);

        new_table
    };

    let mut l1_entry = unsafe { *l1_table }.entries[i1];

    let l2_table = if l1_entry.address() != 0 && l1_entry.is_table() {
        let l2_pa = l1_entry.address();
        l2_pa as *mut TTable<N>
    } else {
        let new_table = alloc_table(pool).expect("pt_pool exhausted");
        let new_pa = new_table as *const _ as u64;

        l1_entry.set_is_valid(true);
        l1_entry.set_is_table();
        l1_entry.set_address(new_pa);

        new_table
    };

    let mut l2_entry = unsafe { *l2_table }.entries[i2];

    let l3_table = if l2_entry.is_valid() {
        let l3_pa = l2_entry.address();
        l3_pa as *mut TTable<N>
    } else {
        let new_table = alloc_table(pool).expect("pt_pool exhausted");
        let new_pa = new_table as *const _ as u64;

        l2_entry.set_is_valid(true);
        l2_entry.set_is_table();
        l2_entry.set_address(new_pa);

        new_table
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
