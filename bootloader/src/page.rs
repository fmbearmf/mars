use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{
        CNTHCTL_EL2, CNTVOFF_EL2, CPACR_EL1, CPTR_EL2, CurrentEL, ELR_EL2, HCR_EL2, ICC_SRE_EL2,
        MAIR_EL1, SCTLR_EL1, SCTLR_EL2, SP, SP_EL1, SP_EL2, SPSR_EL2, TCR_EL1, TTBR0_EL1,
        TTBR0_EL2, TTBR1_EL1,
    },
};
use aarch64_cpu_ext::asm::tlb::{ALLE2, ALLE2IS, VMALLE1, VMALLE1IS, tlbi};
use klib::{
    cache::clean_dcache_range,
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
    if CurrentEL.read(CurrentEL::EL) == 2 {
        dsb(barrier::SY);
        SCTLR_EL2.modify(
            SCTLR_EL2::M::Disable + SCTLR_EL2::C::NonCacheable + SCTLR_EL2::I::NonCacheable,
        );
        isb(barrier::SY);

        HCR_EL2.write(
            HCR_EL2::RW::EL1IsAarch64
                + HCR_EL2::E2H::EnableOsAtEl2
                + HCR_EL2::TGE::EnableTrapGeneralExceptionsToEl2,
        );
        isb(barrier::SY);

        CNTHCTL_EL2.modify(CNTHCTL_EL2::EL1PCEN::SET + CNTHCTL_EL2::EL1PCTEN::SET);
        CNTVOFF_EL2.set(0);
    } else {
        SCTLR_EL1.modify(
            SCTLR_EL1::M::Disable + SCTLR_EL1::C::NonCacheable + SCTLR_EL1::I::NonCacheable,
        );
    }

    // MAIR_EL1.modify(
    //     MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
    //         + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
    //         + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
    //         + MAIR_EL1::Attr2_Normal_Outer::WriteThrough_NonTransient_ReadWriteAlloc
    //         + MAIR_EL1::Attr2_Normal_Inner::WriteThrough_NonTransient_ReadWriteAlloc
    //         + MAIR_EL1::Attr3_Normal_Outer::NonCacheable
    //         + MAIR_EL1::Attr3_Normal_Inner::NonCacheable,
    // );

    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);
    dsb(barrier::SY);
}

pub fn mmu_init(ttbr0: *const TTable<TABLE_ENTRIES>, ttbr1: *const TTable<TABLE_ENTRIES>) {
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

    //TTBR0_EL1.set_baddr(ttbr0 as _);
    TTBR1_EL1.set_baddr(ttbr1 as _);

    if CurrentEL.read(CurrentEL::EL) == 2 {
        tlbi(ALLE2IS);
    } else {
        tlbi(VMALLE1IS);
    }

    dsb(barrier::ISHST);
    isb(barrier::SY);

    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);

    dsb(barrier::SY);
    isb(barrier::SY);
}

pub unsafe fn drop_to_kernel(entry: usize, arg: usize) -> ! {
    use tock_registers::interfaces::{Readable, Writeable};

    let el = CurrentEL.read(CurrentEL::EL);

    if el == 2 {
        // all exceptions masked by default
        SPSR_EL2.write(
            SPSR_EL2::D::Masked
                + SPSR_EL2::A::Masked
                + SPSR_EL2::I::Masked
                + SPSR_EL2::F::Masked
                + SPSR_EL2::M::EL2h,
        );
        isb(barrier::SY);

        ICC_SRE_EL2.write(ICC_SRE_EL2::SRE::SET + ICC_SRE_EL2::ENABLE::SET);
        isb(barrier::SY);

        //SP_EL2.set(SP.get());
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
