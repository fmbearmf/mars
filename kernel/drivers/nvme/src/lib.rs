#![no_std]

extern crate alloc;

pub mod protocol;

#[cfg(all(target_arch = "aarch64", target_os = "none"))]
mod controller;
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
mod dma;
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
pub mod driver;
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
mod registers;
#[cfg(all(target_arch = "aarch64", target_os = "none", feature = "self-test"))]
mod self_test;
