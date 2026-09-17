use aarch64_cpu::{
    asm::{
        self,
        barrier::{self, dsb, isb},
    },
    registers::{CPACR_EL1, DAIF, MAIR_EL1, SCTLR_EL1, TCR_EL1, TPIDR_EL1, TTBR0_EL1},
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, VMALLE1IS, tlbi};
use klib::vm::{TABLE_ENTRIES, TTable};
use tock_registers::interfaces::*;

unsafe extern "C" {
    static __KEND: usize;
}

pub fn init_mmu(ttbr0: Option<*const TTable<TABLE_ENTRIES>>) {
    use log::*;

    TCR_EL1.modify(TCR_EL1::EPD0::DisableTTBR0Walks);
    isb(barrier::SY);

    if let Some(table) = ttbr0 {
        trace!("setting TTBR0_EL1");
        TTBR0_EL1.set_baddr(table as _);
        trace!("set TTBR0_EL1");
    }

    trace!("setting TCR_EL1");
    TCR_EL1.modify(
        TCR_EL1::TBI0::Ignored
            + TCR_EL1::TG0::KiB_16
            + TCR_EL1::SH0::Inner
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::EPD0::EnableTTBR0Walks
            + TCR_EL1::T0SZ.val(16),
    );
    trace!("set TCR_EL1");

    asm::barrier::dsb(asm::barrier::ISH);
    tlbi(VMALLE1IS);
    asm::barrier::dsb(asm::barrier::ISH);
    asm::barrier::isb(asm::barrier::SY);
}

pub fn init_cpu() {
    TPIDR_EL1.set(0);

    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);

    dsb(barrier::SY);
    isb(barrier::SY);
}
