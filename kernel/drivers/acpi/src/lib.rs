#![no_std]

extern crate alloc;

use creusot_std::prelude::*;

use alloc::boxed::Box;
use klib::{
    acpi::xsdp::Xsdp,
    hardware::{
        device::{Device, DeviceId, DeviceNode},
        driver::{DriverDescriptor, DriverError},
    },
};

#[cfg(not(creusot))]
use log::trace;

#[cfg(creusot)]
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

#[cfg(not(creusot))]
pub static DRIVER: DriverDescriptor = DriverDescriptor {
    name: "acpi",
    compatible: &["idek"],
    probe,
};

// creusot is mostly incompatible with dyn
#[cfg(not(creusot))]
fn probe(node: &DeviceNode) -> Result<Box<dyn Device>, DriverError> {
    trace!("probe! {:?}", node.id);

    Err(DriverError::MissingResources)
}

pub struct Acpi<'a> {
    xsdp: &'a Xsdp,
}

impl Device for Acpi<'_> {
    fn shutdown(&self) {}
}

#[ensures(result == x)]
fn f(x: u8) -> u8 {
    x
}
