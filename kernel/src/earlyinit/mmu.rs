use aarch64_cpu::{
    asm::{
        self,
        barrier::{self, isb},
    },
    registers::{CPACR_EL1, DAIF, MAIR_EL1, SCTLR_EL1, TCR_EL1, TTBR0_EL1},
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, tlbi};
use klib::vm::{TABLE_ENTRIES, TTable};
use tock_registers::interfaces::*;

use crate::{busy_loop_ret, earlycon_writeln};

unsafe extern "C" {
    static __KEND: usize;
}

pub fn init_mmu(ttbr0: Option<*const TTable<TABLE_ENTRIES>>) {
    // attr0=device, attr1=normal
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc,
    );

    TCR_EL1.modify(
        TCR_EL1::IPS::Bits_48
            + TCR_EL1::TBI1::Ignored
            + TCR_EL1::TG1::KiB_16
            + TCR_EL1::SH1::Inner
            + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD1::EnableTTBR1Walks
            + TCR_EL1::T1SZ.val(16),
    );

    if let Some(table) = ttbr0 {
        TTBR0_EL1.set_baddr(table as _);
    }

    TCR_EL1.modify(
        TCR_EL1::TBI0::Ignored
            + TCR_EL1::TG0::KiB_16
            + TCR_EL1::SH0::Inner
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD0::EnableTTBR0Walks
            + TCR_EL1::T0SZ.val(16),
    );

    asm::barrier::dsb(asm::barrier::ISHST);

    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    tlbi(VMALLE1);
    asm::barrier::dsb(asm::barrier::SY);
    asm::barrier::isb(asm::barrier::SY);
}

pub fn init_cpu() {
    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);

    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);
}
