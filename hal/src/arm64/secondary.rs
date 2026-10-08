use aarch64_cpu::registers::{
    CPACR_EL1, MAIR_EL1, Readable, SCTLR_EL1, TCR_EL1, TTBR0_EL1, TTBR1_EL1, VBAR_EL1,
};
use core::arch::global_asm;

#[repr(C)]
struct SecondaryBootArgs {
    stack_top: usize,
    entry: usize,
    argument: u32,
    ttbr0: u64,
    ttbr1: u64,
    tcr: u64,
    mair: u64,
    sctlr: u64,
    cpacr: u64,
    vbar: u64,
}

const _: () = {
    assert!(core::mem::size_of::<SecondaryBootArgs>() == 80);
    assert!(core::mem::align_of::<SecondaryBootArgs>() == 8);
    assert!(core::mem::offset_of!(SecondaryBootArgs, stack_top) == 0);
    assert!(core::mem::offset_of!(SecondaryBootArgs, entry) == 8);
    assert!(core::mem::offset_of!(SecondaryBootArgs, argument) == 16);
    assert!(core::mem::offset_of!(SecondaryBootArgs, ttbr0) == 24);
    assert!(core::mem::offset_of!(SecondaryBootArgs, ttbr1) == 32);
    assert!(core::mem::offset_of!(SecondaryBootArgs, tcr) == 40);
    assert!(core::mem::offset_of!(SecondaryBootArgs, mair) == 48);
    assert!(core::mem::offset_of!(SecondaryBootArgs, sctlr) == 56);
    assert!(core::mem::offset_of!(SecondaryBootArgs, cpacr) == 64);
    assert!(core::mem::offset_of!(SecondaryBootArgs, vbar) == 72);
};

#[repr(transparent)]
pub(crate) struct SecondaryBootState(SecondaryBootArgs);

pub(crate) unsafe fn prepare(
    stack_top: usize,
    entry: unsafe extern "C" fn(u32) -> !,
    argument: u32,
) -> SecondaryBootState {
    SecondaryBootState(SecondaryBootArgs {
        stack_top,
        entry: entry as usize,
        argument,
        ttbr0: TTBR0_EL1.get(),
        ttbr1: TTBR1_EL1.get(),
        tcr: TCR_EL1.get(),
        mair: MAIR_EL1.get(),
        sctlr: SCTLR_EL1.get(),
        cpacr: CPACR_EL1.get(),
        vbar: VBAR_EL1.get(),
    })
}

pub(crate) fn entry_address() -> usize {
    smp_trampoline as *const () as usize
}

// this cfg looks stupid, but it's to make `cargo test` work.
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
global_asm!(
    ".global smp_trampoline",
    ".section .text.smp_trampoline, \"ax\"",
    ".align 3",
    "smp_trampoline:",
    "msr daifset, #0xf",
    "mrs x9, CurrentEL",
    "lsr x9, x9, #2",
    "cmp x9, #2",
    "b.ne .L_el2",
    "movz x9, #0x8800, lsl #16",
    "movk x9, #0x0004, lsl #32",
    "msr hcr_el2, x9",
    "isb",
    "mov x9, #0x3c9",
    "msr spsr_el1, x9",
    "isb",
    "adr x9, .L_el2",
    "msr elr_el1, x9",
    "isb",
    "msr hstr_el2, xzr",
    "isb",
    "mov x9, #((3 << 10) | 3)",
    "msr cnthctl_el2, x9",
    "msr cntvoff_el2, xzr",
    "isb",
    "mov x9, #0xf",
    "msr icc_sre_el2, x9",
    "isb",
    "dsb sy",
    "isb",
    "eret",
    ".L_el2:",
    "ldr x9, [x0, #0]",
    "ldr x7, [x0, #8]",
    "ldr w8, [x0, #16]",
    "ldr x1, [x0, #24]",
    "ldr x2, [x0, #32]",
    "ldr x3, [x0, #40]",
    "ldr x4, [x0, #48]",
    "ldr x5, [x0, #56]",
    "ldr x6, [x0, #64]",
    "ldr x10, [x0, #72]",
    "msr ttbr0_el1, x1",
    "isb",
    "msr ttbr1_el1, x2",
    "isb",
    "msr tcr_el1, x3",
    "isb",
    "msr mair_el1, x4",
    "isb",
    "dsb sy",
    "mrs x11, CurrentEL",
    "cmp x11, #8",
    "b.ne .L_tlbi_el1",
    "tlbi alle2",
    "b .L_tlbi_done",
    ".L_tlbi_el1:",
    "tlbi vmalle1",
    ".L_tlbi_done:",
    "dsb sy",
    "isb",
    "msr sctlr_el1, x5",
    "isb",
    "msr cpacr_el1, x6",
    "msr tpidr_el1, xzr",
    "isb",
    "msr vbar_el1, x10",
    "isb",
    "mov sp, x9",
    "mov w0, w8",
    "br x7",
);

unsafe extern "C" {
    fn smp_trampoline();
}
