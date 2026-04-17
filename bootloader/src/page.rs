use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{CPACR_EL1, MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR1_EL1},
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, tlbi};
use klib::{
    pm::page::mapper::AddressTranslator,
    vm::{TABLE_ENTRIES, TTable},
};
use tock_registers::interfaces::*;

#[derive(Debug)]
pub struct UefiAddressTranslator;

// no translation needed
impl AddressTranslator for UefiAddressTranslator {
    fn dmap_to_phys<T>(virt: *mut T) -> u64 {
        virt as _
    }
    fn phys_to_dmap<T>(phys: u64) -> *mut T {
        phys as _
    }
}

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

pub fn mmu_init(table: *const TTable<TABLE_ENTRIES>) {
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
            + TCR_EL1::T1SZ.val(16),
    );

    TTBR1_EL1.set_baddr(table as _);
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    dsb(barrier::ISHST);
    tlbi(VMALLE1);
    dsb(barrier::ISH);
    isb(barrier::SY);
}
