use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{
        CNTHCTL_EL2, CNTVOFF_EL2, CPACR_EL1, CPTR_EL2, CurrentEL, ELR_EL2, HCR_EL2, MAIR_EL1,
        SCTLR_EL1, SPSR_EL2, TCR_EL1, TTBR0_EL1, TTBR1_EL1,
    },
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, tlbi};
use klib::{
    pm::page::mapper::AddressTranslator,
    vm::{TABLE_ENTRIES, TTable},
};
use tock_registers::interfaces::*;

use crate::busy_loop_ret;

#[derive(Debug)]
pub struct UefiAddressTranslator;

// no translation needed
impl AddressTranslator for UefiAddressTranslator {
    fn dmap_to_phys(&self, virt: *mut u8) -> usize {
        virt as _
    }
    fn phys_to_dmap(&self, phys: usize) -> *mut u8 {
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

pub fn mmu_init(ttbr1: *const TTable<TABLE_ENTRIES>) {
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

    TTBR1_EL1.set_baddr(ttbr1 as _);

    tlbi(VMALLE1);
    dsb(barrier::ISHST);
    isb(barrier::SY);

    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    dsb(barrier::SY);
    isb(barrier::SY);
}

pub unsafe fn mmu_init_post_exit() {
    if CurrentEL.read(CurrentEL::EL) == 2 {
        let mair_el1 = MAIR_EL1.get();
        let tcr_el1 = TCR_EL1.get();
        let ttbr0_el1 = TTBR0_EL1.get();
        let ttbr1_el1 = TTBR1_EL1.get();
        let sctlr_el1 = SCTLR_EL1.get();

        HCR_EL2.modify(HCR_EL2::E2H::CLEAR);
        isb(barrier::SY);

        MAIR_EL1.set(mair_el1);
        TCR_EL1.set(tcr_el1);
        TTBR0_EL1.set(ttbr0_el1);
        TTBR1_EL1.set(ttbr1_el1);
        tlbi(VMALLE1);
        dsb(barrier::ISHST);
        isb(barrier::SY);
        SCTLR_EL1.set(sctlr_el1);
        dsb(barrier::SY);
        isb(barrier::SY);
    }
}

pub unsafe fn drop_to_el1(entry: usize, arg: usize) -> ! {
    use tock_registers::interfaces::{Readable, Writeable};

    let el = CurrentEL.read(CurrentEL::EL);

    if el == 2 {
        // 64-bit
        HCR_EL2.write(HCR_EL2::RW::EL1IsAarch64);
        // no FP/SIMD traps
        CPTR_EL2.set(0);

        // passthrough timer
        CNTHCTL_EL2.modify(CNTHCTL_EL2::EL1PCEN::SET + CNTHCTL_EL2::EL1PCTEN::SET);
        CNTVOFF_EL2.set(0);

        // all exceptions masked by default
        SPSR_EL2.write(
            SPSR_EL2::D::Masked
                + SPSR_EL2::A::Masked
                + SPSR_EL2::I::Masked
                + SPSR_EL2::F::Masked
                + SPSR_EL2::M::EL1h,
        );

        ELR_EL2.set(entry as u64);

        dsb(barrier::SY);
        isb(barrier::SY);

        unsafe {
            core::arch::asm!(
                "mov x0, {arg}",
                "eret",
                arg = in(reg) arg,
                options(noreturn)
            )
        };
    } else {
        let f: extern "C" fn(usize) -> ! = unsafe { core::mem::transmute(entry) };
        f(arg)
    }
}
