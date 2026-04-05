use aarch64_cpu::registers::{DAIF, ReadWriteable, Readable, Writeable};
use klib::{
    context::RegisterFileRef, cpu_interface::Mpidr, exception::ExceptionHandler,
    interrupt::InterruptController, timer::timer_irq, vcpu::with_this_cpu,
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
    extern "C" fn fiq_current(register_file: RegisterFileRef) {
        let daif = daif_save();

        with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            if let Some(int) = ack {
                timer_irq();
                gic.end_of_interrupt(int).expect("invalid int id");
            } else {
                // spurious
                return;
            }
        });

        //busy_loop_ret();
        daif_restore(daif);
    }

    extern "C" fn irq_current(register_file: RegisterFileRef) {
        let daif = daif_save();

        with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            if let Some(int) = ack {
                timer_irq();
                gic.end_of_interrupt(int).expect("invalid int id");
            } else {
                // spurious
                return;
            }
        });

        daif_restore(daif);
    }
}
