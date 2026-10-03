use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{
        CNTHCTL_EL2, CNTVOFF_EL2, CPACR_EL1, CurrentEL, DAIF, ELR_EL2, HCR_EL2, ICC_SRE_EL2,
        ReadWriteable, Readable, SCTLR_EL1, SCTLR_EL2, SPSR_EL2, TPIDR_EL1, Writeable,
    },
};
use core::arch::asm;
#[cfg(all(feature = "kernel-entry", target_os = "none"))]
use core::arch::global_asm;

#[cfg(all(feature = "kernel-entry", target_os = "none"))]
#[repr(align(16))]
struct BootstrapStack(#[allow(dead_code)] [u8; 32 * 1024]);

#[cfg(all(feature = "kernel-entry", target_os = "none"))]
static mut BOOTSTRAP_STACK: BootstrapStack = BootstrapStack([0; 32 * 1024]);

// the loader must supply x0. establish the kernel stack before calling rust
// keep this section separate so firmware binaries discard the kernel entry
#[cfg(all(feature = "kernel-entry", target_os = "none"))]
global_asm!(
    ".section .text.hal_kernel_start, \"ax\"",
    ".global _start",
    ".balign 16",
    "_start:",
    "adrp x9, {stack}",
    "add x9, x9, :lo12:{stack}",
    "add x9, x9, {size}",
    "mov sp, x9",
    "bl hal_kernel_entry",
    "brk #0",
    stack = sym BOOTSTRAP_STACK,
    size = const core::mem::size_of::<BootstrapStack>(),
);

pub(crate) unsafe fn leave_firmware() {
    if CurrentEL.read(CurrentEL::EL) == 2 {
        dsb(barrier::SY);
        SCTLR_EL2.modify(SCTLR_EL2::M::Disable);
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
        SCTLR_EL1.modify(SCTLR_EL1::M::Disable);
        isb(barrier::SY);
    }
    CPACR_EL1.modify(
        CPACR_EL1::FPEN::TrapNothing + CPACR_EL1::ZEN::TrapNothing + CPACR_EL1::TTA::NoTrap,
    );
    isb(barrier::SY);
    dsb(barrier::SY);
}

pub(crate) unsafe fn initialize_processor() {
    TPIDR_EL1.set(0);
    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);
    dsb(barrier::SY);
    isb(barrier::SY);
}

pub(crate) unsafe fn enter_kernel(entry: usize, argument: usize) -> ! {
    if CurrentEL.read(CurrentEL::EL) == 2 {
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
        ELR_EL2.set(entry as u64);
        dsb(barrier::SY);
        isb(barrier::SY);
        unsafe { asm!("eret", in("x0") argument, options(noreturn)) }
    } else {
        let entry: unsafe extern "C" fn(usize) -> ! = unsafe { core::mem::transmute(entry) };
        unsafe { entry(argument) }
    }
}

pub(crate) fn stack_pointer() -> usize {
    let pointer: usize;
    unsafe { asm!("mov {}, sp", out(reg) pointer, options(nomem, nostack, preserves_flags)) };
    pointer
}

pub(crate) fn wait() {
    unsafe { asm!("wfe", options(nostack, preserves_flags)) }
}
