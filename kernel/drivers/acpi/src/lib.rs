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
