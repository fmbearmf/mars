use aarch64_cpu::registers::{DAIF, ReadWriteable, Readable, Writeable};
use klib::{
    context::RegisterFileRef, exception::ExceptionHandler, interrupt::InterruptController,
    scheduler::SCHEDULER, timer::timer_irq, vcpu::with_this_cpu,
};

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
                        SCHEDULER.schedule(register_file)
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

        earlycon_writeln!("irq: before scheduling: {:?}", register_file);
        let regs: RegisterFileRef = with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            let regs = match ack {
                Some(int) => {
                    timer_irq();

                    let regs = if int as u64 == cpu.timer_irq {
                        SCHEDULER.schedule(register_file)
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

        earlycon_writeln!("irq: after scheduling: {:?}", regs);

        daif_restore(daif);

        regs
    }
}
