use aarch64_cpu::registers::{
    DAIF, ESR_EL1, FAR_EL1, ReadWriteable, Readable, TTBR0_EL1, Writeable,
};
use klib::{
    context::RegisterFileRef, cpu_interface::Mpidr, exception::ExceptionHandler,
    interrupt::InterruptController, timer::timer_irq, vcpu::with_this_cpu,
};

use crate::{GLOBAL_SCHEDULER, busy_loop_ret};

use super::super::earlycon_writeln;

#[inline(always)]
fn daif_save() -> u64 {
    let daif = DAIF.get();
    DAIF.modify(DAIF::I::Masked + DAIF::F::Masked);
    daif
}

#[inline(always)]
fn daif_restore(daif: u64) {
    DAIF.set(daif);
}

pub struct Exceptions;
impl ExceptionHandler for Exceptions {
    extern "C" fn sync_lower(register_file: RegisterFileRef) -> RegisterFileRef {
        let daif = daif_save();

        let mpidr = with_this_cpu(|cpu| cpu.mpidr);

        earlycon_writeln!(
            "Sync exception from CPU MPIDR={} from lower: {:?} with ESR={:#x} and TTBR0={:#x}",
            mpidr,
            register_file,
            ESR_EL1.get(),
            TTBR0_EL1.get(),
        );

        busy_loop_ret();

        daif_restore(daif);

        register_file
    }

    extern "C" fn irq_lower(register_file: RegisterFileRef) -> RegisterFileRef {
        Self::irq_current(register_file)
    }

    extern "C" fn fiq_current(register_file: RegisterFileRef) -> RegisterFileRef {
        let daif = daif_save();

        earlycon_writeln!("fiq: before scheduling: {:?}", register_file);
        let regs: RegisterFileRef = with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            let regs = match ack {
                Some(int) => {
                    timer_irq();

                    let regs = if int as u64 == cpu.timer_irq {
                        GLOBAL_SCHEDULER.schedule(register_file)
                    } else {
                        register_file
                    };

                    gic.end_of_interrupt(int).expect("invalid int id");
                    regs
                }
                None => register_file,
            };

            regs
        });

        earlycon_writeln!("fiq: after scheduling: {:?}", regs);

        daif_restore(daif);

        regs
    }

    extern "C" fn irq_current(register_file: RegisterFileRef) -> RegisterFileRef {
        let daif = daif_save();

        earlycon_writeln!(
            "irq (CPU {}): before scheduling: {:?}",
            Mpidr::current().affinity_only(),
            register_file
        );
        let regs: RegisterFileRef = with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            let regs = match ack {
                Some(int) => {
                    timer_irq();

                    let regs = if int as u64 == cpu.timer_irq {
                        GLOBAL_SCHEDULER.schedule(register_file)
                    } else {
                        register_file
                    };

                    gic.end_of_interrupt(int).expect("invalid int id");
                    regs
                }
                None => register_file,
            };

            regs
        });

        earlycon_writeln!(
            "irq (CPU {}): after scheduling: {:?}",
            Mpidr::current().affinity_only(),
            regs
        );

        daif_restore(daif);

        regs
    }
}
