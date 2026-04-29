#![no_std]

extern crate alloc;

pub mod acpi;

use alloc::boxed::Box;
use hax_lib::{ensures, opaque, requires, transparent};
use klib::hardware::{
    device::{Device, DeviceNode},
    driver::{DriverDescriptor, DriverError},
};

#[cfg(not(hax))]
use log::trace;

use acpi::xsdp::Xsdp;

#[cfg(hax)]
#[macro_use]
mod macros {
    macro_rules! trace {
        ($($arg:tt)*) => {
            ()
        };
    }
    macro_rules! debug {
        ($($arg:tt)*) => {
            ()
        };
    }
    macro_rules! info {
        ($($arg:tt)*) => {
            ()
        };
    }
    macro_rules! warn {
        ($($arg:tt)*) => {
            ()
        };
    }
    macro_rules! error {
        ($($arg:tt)*) => {
            ()
        };
    }
}

pub static DRIVER: DriverDescriptor = DriverDescriptor {
    name: "acpi",
    compatible: &["idek"],
    probe,
};

pub struct Acpi<'a> {
    xsdp: &'a Xsdp,
}

impl Device for Acpi<'_> {
    fn shutdown(&self) {
        if self.xsdp.checksum() == 1 {
            trace!("blug");
        }
    }
}

fn probe(node: &DeviceNode) -> Result<Box<dyn Device>, DriverError> {
    trace!("probe! {:?}", node.id);

    Err(DriverError::MissingResources)
}
