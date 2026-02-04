use aarch64_cpu::{
    asm::{self, nop},
    registers::{MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::structures::tte::AccessPermission;
use core::{arch::asm, mem::transmute, slice::from_raw_parts_mut};
use mars_kernel::vm::{TABLE_ENTRIES, TTENATIVE, TTable};
use tock_registers::interfaces::*;
use uefi::mem::memory_map::MemoryMapOwned;

use crate::{busy_loop, earlyinit::earlymem::alloc_table};

unsafe extern "C" {
    static __pt_pool_start: usize;
    static __pt_pool_end: usize;
}

pub extern "C" fn init_mmu(load_addr: usize, offset: usize, mmap: &MemoryMapOwned) -> ! {
    // attr0=device, attr1=normal
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    let (LOW_L0, HIGH_L0) = unsafe { setup_tables(load_addr, offset) };

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
    let virt_entry = phys_entry + (offset as usize);
    let init_virt: fn() -> ! = unsafe { transmute(virt_entry) };

    unsafe {
        asm!(
            "add sp, sp, {0}",
            in(reg) offset,
            options(nostack, nomem, preserves_flags)
        );
    }

    nop();
    init_virt()
}

unsafe fn setup_tables(
    load_addr: usize,
    offset: usize,
) -> (
    &'static TTable<TABLE_ENTRIES>,
    &'static TTable<TABLE_ENTRIES>,
) {
    let start = unsafe { &(__pt_pool_start) } as *const usize;
    let end = unsafe { &(__pt_pool_end) } as *const usize;
    let len = unsafe { end.offset_from(start) } as usize;
    let slice = unsafe { from_raw_parts_mut(start as *mut TTable<TABLE_ENTRIES>, len) };

    let LOW_L0 = alloc_table(slice).unwrap();
    let LOW_L1 = alloc_table(slice).unwrap();
    let LOW_L2 = alloc_table(slice).unwrap();
    let HIGH_L0 = alloc_table(slice).unwrap();
    let HIGH_L1 = alloc_table(slice).unwrap();
    let HIGH_L2 = alloc_table(slice).unwrap();
    let HIGH_L2_MMIO = alloc_table(slice).unwrap();

    busy_loop();

    unsafe {
        // [0] -> L1
        LOW_L0.entries[0] = TTENATIVE::new_table(LOW_L1 as *const _ as u64);
        // [0] -> L2
        LOW_L1.entries[0] = TTENATIVE::new_table(LOW_L2 as *const _ as u64);
    }

    let index = (load_addr >> 25) as usize;

    unsafe {
        let mut block = TTENATIVE::new_block(load_addr as u64);
        block.set_attr_index(1);
        block.set_access_permission(
            aarch64_cpu_ext::structures::tte::AccessPermission::PrivilegedReadWrite,
        );
        block.set_executable(false);
        block.set_privileged_executable(true);

        LOW_L2.entries[index] = block;
    }

    unsafe {
        // L0[1] -> L1
        HIGH_L0.entries[1] = TTENATIVE::new_table(HIGH_L1 as *const _ as u64);
        // L1[0] -> L2
        HIGH_L1.entries[0] = TTENATIVE::new_table(HIGH_L2 as *const _ as u64);
        // L1[1] -> MMIO L2
        HIGH_L1.entries[1] = TTENATIVE::new_table(HIGH_L2_MMIO as *const _ as u64);
    }

    for i in 0..4 {
        let block_phys = load_addr + (i * 32 * 1024 * 1024);
        let mut block = TTENATIVE::new_block(block_phys as u64);
        block.set_attr_index(1);
        block.set_access_permission(AccessPermission::PrivilegedReadWrite);
        block.set_executable(false);
        block.set_privileged_executable(true);
        unsafe {
            HIGH_L2.entries[i as usize] = block;
        }
    }

    {
        let block_phys = 0x4000_0000;
        let mut block = TTENATIVE::new_block(block_phys);
        block.set_attr_index(1);
        block.set_access_permission(AccessPermission::PrivilegedReadOnly);
        block.set_executable(false);
        block.set_privileged_executable(false);
        unsafe {
            HIGH_L2.entries[TABLE_ENTRIES - 1] = block;
        }
    }

    // on QEMU all MMIO is in the first GB of memory.
    // 32MiB (L2 Pages) * 32 (entries) = 1GiB
    for i in 0..32 {
        let block_phys = i * 32 * 1024 * 1024;
        let mut block = TTENATIVE::new_block(block_phys);
        block.set_attr_index(0);
        block.set_access_permission(AccessPermission::PrivilegedReadWrite);
        block.set_executable(false);
        block.set_privileged_executable(false);
        unsafe {
            HIGH_L2_MMIO.entries[i as usize] = block;
        }
    }

    (LOW_L0, HIGH_L0)
}
