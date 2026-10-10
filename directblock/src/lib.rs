#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

#[cfg(test)]
pub mod tests;

pub mod bio;
pub mod buffer;
pub mod device;
pub mod graph;

use alloc::{boxed::Box, vec::Vec};
use core::fmt;
use vstd::prelude::*;

use crate::{buffer::IoBuffer, device::Flight};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum IoError {
    NoDevice,
    Io,
    OutOfBounds,
    Unsupported,
    GeometryMismatch,
    ReadOnly,
    InvalidBuffer,

    // lifecycle/rss
    NotReady,
    Quiescing,
    Offline,
    DeviceFailed,
    Busy,
    ResourceExhausted,

    // graph
    InvalidTopology,
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

pub type IoResult = Result<(), IoError>;

pub struct Completion {
    cb: Box<dyn FnOnce(IoResult, Option<IoBuffer>) + Send>,
    leases: Vec<Flight>,
}

impl Completion {
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce(IoResult, Option<IoBuffer>) + Send + 'static,
    {
        Self {
            cb: Box::new(f),
            leases: Vec::new(),
        }
    }

    pub(crate) fn attach(&mut self, lease: Flight) {
        self.leases.push(lease);
    }

    pub(crate) fn detach(&mut self) {
        // most recent admission
        self.leases.pop();
    }

    pub fn complete(self, result: IoResult, buf: Option<IoBuffer>) {
        let Self { cb, leases } = self;

        cb(result, buf);
        drop(leases);
    }
}
