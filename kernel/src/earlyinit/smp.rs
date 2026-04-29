use core::arch::{asm, naked_asm};

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{CPACR_EL1, DAIF, ReadWriteable, TCR_EL1, TTBR0_EL1, Writeable},
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, tlbi};
use klib::{
    cpu_interface::{Mpidr, SecondaryBootArgs},
    interrupt::InterruptController,
    timer::{init_timer, timer_rearm},
    vcpu::{CpuState, vcpu_fsm_advance},
    vm::phys_addr_to_dmap,
};

use super::super::busy_loop;

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
        "ldr x8, [x0, #56]", // cpudescriptor
        "ldr x9, [x0, #64]", // gicd
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

pub extern "C" fn secondary_init(context_phys: *const SecondaryBootArgs) -> ! {
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

    DAIF.write(DAIF::D::Unmasked + DAIF::A::Unmasked + DAIF::I::Unmasked + DAIF::F::Unmasked);
    isb(barrier::SY);

    TTBR0_EL1.set_baddr(0);
    TCR_EL1.modify(TCR_EL1::EPD0::DisableTTBR0Walks);
    tlbi(VMALLE1);
    dsb(barrier::ISH);
    isb(barrier::SY);

    let mpidr = Mpidr::current();
    let context_ptr = phys_addr_to_dmap(context_phys as u64) as *const SecondaryBootArgs;
    let context = unsafe { &*context_ptr };

    let desc = unsafe { &*(context.cpu_desc) };

    let mut gic = desc.gic.expect("`None` gic");

    gic.init().expect("gic init fail");
    gic.enable_interrupt(desc.timer_irq as u32)
        .expect("error enabling timer IRQ");

    init_timer();
    timer_rearm();

    let new_state = vcpu_fsm_advance(mpidr.affinity_only() as usize);
    assert_eq!(new_state, CpuState::Done);

    busy_loop()
}
