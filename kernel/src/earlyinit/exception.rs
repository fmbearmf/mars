use hal::{context::Context, exception::Exception};
use klib::{interrupt::singleton::get_interrupt_controller, scheduler::GLOBAL_SCHEDULER};
use log::error;
use mars_generic_timer_driver::timer::{
    TIMER, TimerError, timer_disarm, timer_rearm, timer_schedule,
};

/// safety:
/// enter with interrupts masked and a live exception frame
pub unsafe fn handle(exception: Exception, context: &mut Context) {
    struct ExceptionDepth(&'static klib::per_cpu::PerCpuData);
    impl Drop for ExceptionDepth {
        fn drop(&mut self) {
            self.0
                .exception_depth
                .fetch_sub(1, core::sync::atomic::Ordering::Release);
        }
    }

    let cpu = klib::per_cpu::PerCpu::try_local();
    let _depth = cpu.map(|cpu| {
        let depth = cpu
            .exception_depth
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        let guard = ExceptionDepth(cpu);
        assert_eq!(depth, 0, "nested exceptions are unsupported");
        guard
    });

    match exception {
        Exception::Reschedule => {
            assert!(
                cpu.is_some(),
                "reschedule exception before cpu registration"
            );
            GLOBAL_SCHEDULER.schedule(context)
        }
        Exception::Interrupt => {
            assert!(cpu.is_some(), "interrupt before cpu registration");
            interrupt(context)
        }
        Exception::Fault { user } => {
            if let Some(cpu) = cpu {
                error!("fault on cpu {}, user={}: {:?}", cpu.id, user, context);
            } else {
                error!("fault on unknown cpu, user={}: {:?}", user, context);
            }
            panic!("unhandled execution fault");
        }
        Exception::Fatal => {
            if let Some(cpu) = cpu {
                panic!("fatal processor exception on cpu {}", cpu.id);
            } else {
                panic!("fatal processor exception on unknown cpu");
            }
        }
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
