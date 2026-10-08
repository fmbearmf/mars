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

    assert_eq!(
        node.resources.len(),
        1,
        "Generic Timer invoked with more than one resource!"
    );

    info!(
        "ARMv8 Generic Timer: Using interrupts: {:?}",
        node.resources.as_slice()
    );

    // unwrap because the length was just checked
    if let Resource::Irq(irq) = node.resources.iter().next().unwrap() {
        info!("ARMv8 Generic Timer: Setting global timer IRQ to {}", irq);
        TIMER
            .set_irq((*irq) as _)
            .expect("couldn't borrow global TIMER");
    } else {
        panic!("Generic Timer called with non-IRQ resource");
    }

    // primary initialization is no different from secondary
    secondary_handle(node, enable_irq, _disable_irq)
}
