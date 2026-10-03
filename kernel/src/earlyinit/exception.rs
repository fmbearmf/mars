use hal::{context::Context, exception::Exception};
use klib::{interrupt::singleton::get_interrupt_controller, scheduler::GLOBAL_SCHEDULER, this_cpu};
use log::error;
use mars_generic_timer_driver::timer::{
    TIMER, TimerError, timer_disarm, timer_rearm, timer_schedule,
};

/// safety:
/// enter with interrupts masked and a live exception frame
pub unsafe fn handle(exception: Exception, context: &mut Context) {
    match exception {
        Exception::Reschedule => GLOBAL_SCHEDULER.schedule(context),
        Exception::Interrupt => interrupt(context),
        Exception::Fault { user } => {
            error!(
                "fault on cpu {}, user={}: {:?}",
                this_cpu!().id,
                user,
                context
            );
            panic!("unhandled execution fault");
        }
        Exception::Fatal => panic!("fatal processor exception"),
    }
}

fn interrupt(context: &mut Context) {
    let controller = get_interrupt_controller();
    if let Some(id) = controller.acknowledge_interrupt().expect("ack failure") {
        timer_disarm();
        timer_rearm();
        let is_timer = match TIMER.get_irq() {
            Ok(irq) => irq.map(u32::from) == Some(id),
            Err(TimerError::CouldntBorrow) => {
                log::warn!("interrupt arrived before timer initialization");
                false
            }
            Err(error) => {
                log::warn!("timer irq unavailable: {:?}", error);
                false
            }
        };
        if is_timer {
            GLOBAL_SCHEDULER.schedule(context);
        } else {
            controller
                .on_interrupt(id)
                .expect("interrupt dispatch failed");
        }
        controller
            .end_of_interrupt(id)
            .expect("invalid interrupt id");
    }
    timer_schedule();
}
