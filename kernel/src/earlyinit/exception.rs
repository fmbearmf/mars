use core::sync::atomic::Ordering;

use aarch64_cpu::registers::{DAIF, ESR_EL1, ReadWriteable, Readable, TTBR0_EL1, Writeable};
use klib::{
    context::RegisterFileRef,
    cpu_interface::CpuTopologyId,
    exception::ExceptionHandler,
    interrupt::InterruptController,
    this_cpu,
    timer::{timer_disarm, timer_rearm, timer_schedule},
};
use log::{error, trace};

use crate::{GLOBAL_SCHEDULER, busy_loop_ret, interrupt::get_interrupt_controller};

use super::super::earlycon_writeln;

#[inline(always)]
fn daif_save() -> u64 {
    let daif = DAIF.get();
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

        let current = this_cpu!();

        error!(
            "Sync exception from CPU ID={} from lower: {:?} with ESR={:#x} and TTBR0={:#x}",
            current.id,
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

        trace!("fiq: before scheduling: {:?}", register_file);
        let regs: RegisterFileRef = {
            let gic = get_interrupt_controller();

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            let regs = match ack {
                Some(int) => {
                    timer_disarm();
                    timer_rearm();

                    let regs = if int == this_cpu!().timer_irq.load(Ordering::Relaxed) as u32 {
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
        };

        trace!("fiq: after scheduling: {:?}", regs);

        daif_restore(daif);
        timer_schedule();

        regs
    }

    extern "C" fn irq_current(register_file: RegisterFileRef) -> RegisterFileRef {
        let daif = daif_save();

        trace!(
            "irq (CPU {}): before scheduling: {:?}",
            CpuTopologyId::current().to_mpidr(),
            register_file
        );
        let regs: RegisterFileRef = {
            let gic = get_interrupt_controller();

            let ack = gic.acknowledge_interrupt().expect("ack failure");

            let regs = match ack {
                Some(int) => {
                    timer_disarm();
                    timer_rearm();

                    let regs = if int == this_cpu!().timer_irq.load(Ordering::Relaxed) as u32 {
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
        };

        trace!(
            "irq (CPU {}): after scheduling: {:?}",
            CpuTopologyId::current().to_mpidr(),
            regs
        );

        daif_restore(daif);
        timer_schedule();

        regs
    }
}
