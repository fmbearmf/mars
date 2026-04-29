#![no_std]

#[cfg(hax)]
#[path = "./shim/mod.rs"]
pub(self) mod shim;

#[cfg(not(hax))]
pub(self) mod shim {
    pub use zerocopy::*;

    extern crate self as zerocopy;

    #[cfg(feature = "derive")]
    pub use zerocopy_derive::*;
}

pub use self::shim::*;
