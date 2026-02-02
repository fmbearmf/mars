use aarch64_cpu::{
    asm::{self, nop},
    registers::{MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::structures::tte::AccessPermission;
use core::{arch::asm, mem::transmute};
use mars_kernel::vm::{TABLE_ENTRIES, TTENATIVE, TTable};
use tock_registers::interfaces::*;

unsafe extern "C" {
    pub static KERNEL_OFFSET: u64;
    pub static KERNEL_LOAD_PHYS_RAW: u64;
    //pub static KERNEL_LOAD_VIRT_RAW: u64;
}

// static mut IS vulnerable to race conditions, but these are only accessed from a single core
// LOW meaning TTBR0, the lower part of the address space
// L0 only has 2 entries
#[unsafe(link_section = ".reclaimable.tables")]
static mut LOW_L0: TTable<2> = TTable {
    entries: [TTENATIVE::invalid(); 2],
};

#[unsafe(link_section = ".reclaimable.tables")]
static mut LOW_L1: TTable<TABLE_ENTRIES> = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

#[unsafe(link_section = ".reclaimable.tables")]
static mut LOW_L2: TTable<TABLE_ENTRIES> = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

// conversely HIGH refers to TTBR1
#[unsafe(link_section = ".reclaimable.tables")]
static mut HIGH_L0: TTable<2> = TTable {
    entries: [TTENATIVE::invalid(); 2],
};

#[unsafe(link_section = ".reclaimable.tables")]
static mut HIGH_L1: TTable<TABLE_ENTRIES> = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

#[unsafe(link_section = ".reclaimable.tables")]
static mut HIGH_L2: TTable<TABLE_ENTRIES> = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

#[unsafe(link_section = ".reclaimable.tables")]
static mut HIGH_L2_MMIO: TTable<TABLE_ENTRIES> = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

pub extern "C" fn init_mmu(load_addr: u64, offset: u64) -> ! {
    // attr0=device, attr1=normal
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    unsafe {
        setup_tables(load_addr);
    }

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

    TTBR0_EL1.set(&raw mut LOW_L0 as *const _ as u64);
    TTBR1_EL1.set(&raw mut HIGH_L0 as *const _ as u64);
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

unsafe fn setup_tables(load_addr: u64) {
    unsafe {
        // [0] -> L1
        LOW_L0.entries[0] = TTENATIVE::new_table(&raw mut LOW_L1 as *const _ as u64);
        // [0] -> L2
        LOW_L1.entries[0] = TTENATIVE::new_table(&raw mut LOW_L2 as *const _ as u64);
    }

    // align down to 32mib
    let mut phys_base = load_addr - (load_addr % (32 * 1024 * 1024));

    let index = (phys_base >> 25) as usize;

    unsafe {
        let mut block = TTENATIVE::new_block(phys_base);
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
        HIGH_L0.entries[1] = TTENATIVE::new_table(&raw mut HIGH_L1 as *const _ as u64);
        // L1[0] -> L2
        HIGH_L1.entries[0] = TTENATIVE::new_table(&raw mut HIGH_L2 as *const _ as u64);
        // L1[1] -> MMIO L2
        HIGH_L1.entries[1] = TTENATIVE::new_table(&raw mut HIGH_L2_MMIO as *const _ as u64);
    }

    for i in 0..4 {
        let block_phys = phys_base + (i * 32 * 1024 * 1024);
        let mut block = TTENATIVE::new_block(block_phys);
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
}
