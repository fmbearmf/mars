#![no_std]

pub(self) mod common;
pub use common::*;

#[cfg(target_arch = "aarch64")]
#[path = "arm64/mod.rs"]
pub(self) mod arch;

#[cfg(not(target_arch = "aarch64"))]
compile_error!("Missing HAL for target architecture.");
