//! Hax doesn't support foreign types.
//! Doing so creates unresolved references in the outputted F*.
//! These shims exist to make sure that F* has placeholders.

pub mod error;

use error::CastError;
use hax_lib::{attributes, ensures, opaque};

#[cfg(feature = "derive")]
pub use mars_zerocopy_derive::*;

#[cfg(feature = "derive")]
//extern crate mars_zerocopy_derive as

pub trait FromZeroes {}

#[attributes]
pub trait FromBytes {
    #[requires(slice.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn ref_from_prefix(slice: &[u8]) -> Result<(&Self, &[u8]), CastError<&[u8], Self>>
    where
        Self: KnownLayout + Immutable;

    #[requires(source.len() as usize >= core::mem::size_of::<Self>())]
    #[ensures(|result| result.is_ok())]
    fn read_from_prefix(source: &[u8]) -> Result<(Self, &[u8]), CastError<&[u8], Self>>
    where
        Self: Sized;
}

pub trait KnownLayout {
    fn kl_noop(); // Hax bug: marker traits don't work
}
pub trait Unaligned {
    fn ul_noop(); // Hax bug: marker traits don't work
}
pub trait Immutable {
    fn im_noop() {} // Hax bug: marker traits don't work
}
