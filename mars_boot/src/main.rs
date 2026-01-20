#![no_std]
#![no_main]

use core::{arch::asm, mem::transmute, ops::Add};

use aarch64_cpu::{
    asm::{self, nop, wfe},
    registers::{MAIR_EL1, SCTLR_EL1, SP, TCR_EL1, TTBR0_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::structures::tte::TTE16K48 as TTENATIVE;
use tock_registers::interfaces::*;

extern crate core;
extern crate semihosting;

unsafe extern "C" {
    static KERNEL_OFFSET: u64;
    static KERNEL_LOAD_PHYS_RAW: u64;
    static KERNEL_LOAD_VIRT_RAW: u64;
}

// num. of entries per table, which is equal to the granule size divided by 8.
const TABLE_ENTRIES: usize =
    aarch64_cpu_ext::structures::tte::block_sizes::granule_16k::LEVEL3_PAGE_SIZE / 8usize;

#[repr(C, align(16384))]
pub struct TTable {
    pub entries: [TTENATIVE; TABLE_ENTRIES],
}

// static mut is vulnerable to race conditions, but these are only accessed from a single core
// so who cares

// LOW means TTBR0, the lower part of the address space (which will be partially identity mapped for the transition to virtual memory)
static mut LOW_L0: TTable = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};
static mut LOW_L1: TTable = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};
static mut LOW_L2: TTable = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

// conversely HIGH refers to TTBR1
static mut HIGH_L0: TTable = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};
static mut HIGH_L1: TTable = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};
static mut HIGH_L2: TTable = TTable {
    entries: [TTENATIVE::invalid(); TABLE_ENTRIES],
};

fn busy_loop() -> ! {
    loop {
        wfe();
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    core::arch::naked_asm!(
        "ldr x0, ={offset}",
        "ldr x30, =__boot_stack_top",
        "sub x30, x30, x0",
        "mov sp, x30",
        //
        "mrs x1, mpidr_el1", // core ID
        "and x1, x1, #0xFF", // check Aff0 (core ID)
        "cbnz x1, 1f",
        "ldr x1, =__bss_start",
        "ldr x2, =__bss_end",
        "sub x1, x1, x0",
        "sub x2, x2, x0",
        "2: cmp x1, x2",
        "b.ge 3f",
        "str xzr, [x1], #8",
        "b 2b",
        "3: ldr x0, ={lma}",
        "ldr x1, ={offset}",
        "bl {setup}",
        "1: wfe",
        "b 1b",
        offset = sym KERNEL_OFFSET,
        setup = sym init_mmu,
        lma = sym KERNEL_LOAD_PHYS_RAW,
    );
}

extern "C" fn init_mmu(load_addr: u64, offset: u64) -> ! {
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

    unsafe extern "Rust" {
        fn arm_init() -> !;
    }
    let phys_entry = arm_init as *const () as usize;
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
    let phys_base = load_addr - (load_addr % (32 * 1024 * 1024));
    let index = (phys_base >> 25) as usize;

    unsafe {
        let mut block = TTENATIVE::new_block(phys_base);
        block.set_attr_index(1);
        block.set_access_permission(
            aarch64_cpu_ext::structures::tte::AccessPermission::PrivilegedReadWrite,
        );
        block.set_executable(true);
        block.set_privileged_executable(true);

        LOW_L2.entries[index] = block;
    }

    unsafe {
        HIGH_L0.entries[1] = TTENATIVE::new_table(&raw mut HIGH_L1 as *const _ as u64);
        HIGH_L1.entries[0] = TTENATIVE::new_table(&raw mut HIGH_L2 as *const _ as u64);
    }

    for i in 0..4 {
        let block_phys = phys_base + (i * 32 * 1024 * 1024);
        let mut block = TTENATIVE::new_block(block_phys);
        block.set_attr_index(1);
        block.set_access_permission(
            aarch64_cpu_ext::structures::tte::AccessPermission::PrivilegedReadWrite,
        );
        block.set_executable(true);
        block.set_privileged_executable(true);
        unsafe {
            HIGH_L2.entries[i as usize] = block;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn arm_init() {
    busy_loop();
}
