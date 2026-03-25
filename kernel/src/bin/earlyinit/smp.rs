use core::arch::{asm, naked_asm};

use aarch64_cpu::{
    asm::barrier::{self, isb},
    registers::{CPACR_EL1, DAIF, ReadWriteable, Readable, Writeable},
};
use klib::cpu_interface::{Mpidr, SecondaryBootArgs};

use crate::{busy_loop, earlycon_writeln};

#[unsafe(naked)]
pub unsafe extern "C" fn secondary_entry(context: *const SecondaryBootArgs) -> ! {
    naked_asm!(
        "ldr x1, [x0, #0]",  // ttbr0
        "ldr x2, [x0, #8]",  // ttbr1
        "ldr x3, [x0, #16]", // tcr
        "ldr x4, [x0, #24]", // mair
        "ldr x5, [x0, #32]", // stack_top_virt
        "ldr x6, [x0, #40]", // entry_virt
        "ldr x7, [x0, #48]", // sctlr
        //
        "msr ttbr0_el1, x1",
        "msr ttbr1_el1, x2",
        "msr tcr_el1, x3",
        "msr mair_el1, x4",
        //
        "dsb ish",
        "tlbi vmalle1is",
        "dsb sy",
        "isb",
        //
        "msr sctlr_el1, x7",
        "isb",
        //
        "mov sp, x5",
        "br x6",
    )
}

pub fn secondary_init() -> ! {
    unsafe {
        asm!(
            "adr {x}, vector_table_el1",
            "msr vbar_el1, {x}",
            x = out(reg) _,
            options(nomem, nostack),
        );
    }

    CPACR_EL1.modify(CPACR_EL1::FPEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::ZEN::TrapNothing);
    CPACR_EL1.modify(CPACR_EL1::TTA::NoTrap);
    isb(barrier::SY);

    DAIF.write(DAIF::D::Masked + DAIF::A::Masked + DAIF::I::Masked + DAIF::F::Masked);
    isb(barrier::SY);

    earlycon_writeln!(
        "hello from secondary cpu mpidr={}",
        Mpidr::current().affinity_only(),
    );

    busy_loop()
}
