#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use klib::{
    acpi::xsdp::Xsdp,
    hardware::{
        device::{Device, DeviceId, DeviceNode},
        driver::{DriverDescriptor, DriverError},
    },
};
use log::trace;

pub static DRIVER: DriverDescriptor = DriverDescriptor {
    name: "acpi",
    compatible: &["idek"],
    probe,
};

pub struct Acpi<'a> {
    xsdp: &'a Xsdp,
}

impl Device for Acpi<'_> {
    fn shutdown(&self) {}
}

fn probe(node: &DeviceNode) -> Result<Box<dyn Device>, DriverError> {
    trace!("probe! {:?}", node.id);

    Err(DriverError::MissingResources)
}
