use core::ptr::NonNull;

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{CPACR_EL1, MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::structures::tte::{AccessPermission, Shareability};
use klib::vm::{
    PAGE_MASK, PAGE_SHIFT, PAGE_SIZE, TABLE_ENTRIES, TTENATIVE, TTable, TTableUEFI,
    map::TableAllocator,
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

    TCR_EL1.modify(
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
