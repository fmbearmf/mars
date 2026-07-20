#![no_std]

pub mod timer;

use klib::{
    hardware::{
        device::{DeviceNode, IrqFn},
        resource::Resource,
    },
    this_cpu,
};
use timer::*;

extern crate alloc;

pub fn secondary_handle(node: &DeviceNode, enable_irq: IrqFn, _disable_irq: IrqFn) {
    use log::*;

    let id = this_cpu!().id;

    init_timer();
    for resource in node
        .resources
        .iter()
        .filter(|n| matches!(n, Resource::Irq(_)))
    {
        enable_irq(resource).expect("failed to enable IRQ in timer handler");
    }
    info!("ARMv8 Generic Timer: Enabled on core {}.", id);
    timer_rearm();
    timer_schedule();
}

pub fn handle(node: &DeviceNode, enable_irq: IrqFn, _disable_irq: IrqFn) {
    use log::*;

    info!(
        "ARMv8 Generic Timer: Using interrupts: {:?}",
        node.resources.as_slice()
    );
    // primary initialization is no different from secondary
    secondary_handle(node, enable_irq, _disable_irq)
}
