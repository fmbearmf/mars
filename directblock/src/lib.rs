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
pub mod memory;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::{array, fmt};
use vstd::prelude::*;

use crate::{buffer::IoBuffer, device::Provider};

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

type CompletionFn = Box<dyn FnOnce(IoResult, Option<IoBuffer>) + Send>;

/// max number of providers through which a BIO can pass
pub const MAX_GRAPH_DEPTH: usize = 8;

pub struct Completion {
    cb: Option<CompletionFn>,
    flights: [Option<Arc<Provider>>; MAX_GRAPH_DEPTH],
    depth: usize,
}

impl Completion {
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce(IoResult, Option<IoBuffer>) + Send + 'static,
    {
        Self {
            cb: Some(Box::new(f)),
            flights: array::from_fn(|_| None),
            depth: 0,
        }
    }

    /// register a provider admission
    fn push(&mut self, provider: Arc<Provider>) -> Result<(), IoError> {
        if self.depth == MAX_GRAPH_DEPTH {
            return Err(IoError::ResourceExhausted);
        }

        self.flights[self.depth] = Some(provider);
        self.depth += 1;

        Ok(())
    }

    /// undo the most recent admission
    fn pop(&mut self) {
        if self.depth == 0 {
            return;
        }

        self.depth -= 1;

        if let Some(provider) = self.flights[self.depth].take() {
            provider.release();
        }
    }

    /// release admission and invoke callback
    pub fn complete(mut self, result: IoResult, buf: Option<IoBuffer>) {
        while self.depth != 0 {
            self.pop();
        }

        let callback = self.cb.take().expect("completion already consumed");

        callback(result, buf);
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        while self.depth != 0 {
            self.pop();
        }
    }
}
