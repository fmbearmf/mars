#![no_std]
#![feature(proc_macro_hygiene)]

#[cfg(hax)]
#[hax_lib::exclude]
include!("./shim/mod.rs");

#[cfg(not(hax))]
include!("./dummy.rs");
