use core::arch::{asm, naked_asm};

use aarch64_cpu::{
    asm::barrier::{self, dsb, isb},
    registers::{
        CNTFRQ_EL0, CNTV_CTL_EL0, CNTV_CVAL_EL0, CNTV_TVAL_EL0, CNTVCT_EL0, CNTVOFF_EL2, CPACR_EL1,
        DAIF, HFGRTR_EL2::ISR_EL1, ICC_SRE_EL2, ReadWriteable, Readable, TCR_EL1, TTBR0_EL1,
        Writeable,
    },
};
use aarch64_cpu_ext::asm::tlb::{VMALLE1, tlbi};
use klib::{
    cpu_interface::{Arm64InterruptInterface, Mpidr, SecondaryBootArgs},
    interrupt::{
        GicdRegisters, GicrRdRegisters, GicrSgiRegisters, InterruptController,
        gicv3::{
            GicV3,
            registers::{GICD_TYPER, icc_pmr_el1::ICC_PMR_EL1, icc_sre_el1::ICC_SRE_EL1},
        },
    },
    vcpu::{CpuState, vcpu_fsm_advance},
    vm::phys_addr_to_dmap,
};

use crate::{busy_loop, busy_loop_ret, earlycon_writeln};

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

    earlycon_writeln!(
        "hello from secondary cpu mpidr={} timer_irq={}",
        mpidr.affinity_only(),
        desc.timer_irq
    );

    let interrupt_controller = Arm64InterruptInterface {};

    let gicr = desc
        .gicr
        .expect("`None` GIC frame passed to secondary core");

    earlycon_writeln!("rd: {:p}", gicr.rd);
    earlycon_writeln!("sgi: {:p}", gicr.sgi);

    let gicd = unsafe { &*context.gicd };

    earlycon_writeln!(
        "gicd: ctlr={:#x} typer={:#x}",
        gicd.CTLR.get(),
        gicd.TYPER.get()
    );

    let security_extension = gicd.TYPER.matches_all(GICD_TYPER::SecurityExtn::SET);
    earlycon_writeln!("security extensions: {}", security_extension);

    earlycon_writeln!("gicd: {:p}", gicd);

    let mut gic = GicV3::new(gicd, gicr.rd, gicr.sgi, interrupt_controller);

    gic.init().expect("gic init fail");

    gic.enable_interrupt(27).expect("error enabling int27");

    let freq = CNTFRQ_EL0.get();

    earlycon_writeln!("timer ticks per second: {}", freq);
    earlycon_writeln!("timer compare value: {}", CNTV_CVAL_EL0.get());
    earlycon_writeln!("timer value: {}", CNTVCT_EL0.get());
    earlycon_writeln!("timer tval: {}", CNTV_TVAL_EL0.get());

    let future_time = CNTVCT_EL0.get() + (2 * freq);

    earlycon_writeln!("future time: {}", future_time);

    CNTV_CTL_EL0.set(0);
    isb(barrier::SY);
    CNTV_CVAL_EL0.set(future_time);
    isb(barrier::SY);

    CNTV_CTL_EL0.modify(CNTV_CTL_EL0::ENABLE::SET + CNTV_CTL_EL0::IMASK::CLEAR);
    isb(barrier::SY);

    let new_state = vcpu_fsm_advance(mpidr.affinity_only() as usize);
    assert_eq!(new_state, CpuState::Done);

    loop {
        let ctl = CNTV_CTL_EL0.matches_all(CNTV_CTL_EL0::ISTATUS::SET);

        if ctl {
            let ispendr0 = gicr.sgi.ISPENDR0.get();
            let isenabler0 = gicr.sgi.ISENABLER0.get();
            let igroupr0 = gicr.sgi.IGROUPR0.get();
            let waker = gicr.rd.WAKER.get();

            busy_loop_ret();
        }
        isb(barrier::SY);
    }
}
