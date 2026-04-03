use aarch64_cpu::registers::{DAIF, FAR_EL1, ReadWriteable, Readable, VBAR_EL1, Writeable};
use klib::{
    context::RegisterFileRef, cpu_interface::Mpidr, exception::ExceptionHandler,
    interrupt::InterruptController, vcpu::with_this_cpu,
};

use super::super::{busy_loop_ret, earlycon_writeln};

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
        earlycon_writeln!(
            "current EL FIQ from CPU MPIDR={} (FAR: {:#x}) from current EL: {:?}",
            Mpidr::current().affinity_only(),
            FAR_EL1.get(),
            register_file
        );
        let daif = daif_save();

        with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            if let Some(int) = ack {
                earlycon_writeln!("fiq: cpu={} int={}", Mpidr::current().affinity_only(), int);
                gic.end_of_interrupt(int).expect("invalid int id");
            } else {
                // spurious
                return;
            }
        });

        daif_restore(daif);
    }

    extern "C" fn irq_current(register_file: RegisterFileRef) {
        earlycon_writeln!(
            "current EL IRQ from CPU MPIDR={} (FAR: {:#x}) from current EL: {:?}",
            Mpidr::current().affinity_only(),
            FAR_EL1.get(),
            register_file
        );
        let daif = daif_save();

        with_this_cpu(|cpu| {
            let mut gic = cpu.gic.expect("`None` GIC");

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            if let Some(int) = ack {
                earlycon_writeln!("irq: cpu={} int={}", Mpidr::current().affinity_only(), int);
                gic.end_of_interrupt(int).expect("invalid int id");
            } else {
                // spurious
                return;
            }
        });

        daif_restore(daif);
    }
}
