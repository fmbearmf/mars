use core::marker::PhantomData;

use crate::align::{Alignment, BufferAccess, NoData, ReadOnly, WriteOnly};

pub trait BlockOperation: Send + Sync + 'static {
    /// repeating has no side-effects?
    const IS_IDEMPOTENT: bool;
    /// permanent change to media?
    const MODIFIES_MEDIA: bool;

    /// access pattern
    type RequiredAccess: BufferAccess;
}

#[derive(Debug, Copy, Clone)]
pub struct Read<A: Alignment>(PhantomData<A>);
impl<A: Alignment> BlockOperation for Read<A> {
    const IS_IDEMPOTENT: bool = true;
    const MODIFIES_MEDIA: bool = false;
    type RequiredAccess = WriteOnly;
}

#[derive(Debug, Copy, Clone)]
pub struct Write<A: Alignment>(PhantomData<A>);
impl<A: Alignment> BlockOperation for Write<A> {
    const IS_IDEMPOTENT: bool = false;
    const MODIFIES_MEDIA: bool = true;
    type RequiredAccess = ReadOnly;
}

#[derive(Debug, Copy, Clone)]
pub struct Flush;
impl BlockOperation for Flush {
    const IS_IDEMPOTENT: bool = true;
    const MODIFIES_MEDIA: bool = true;
    type RequiredAccess = NoData;
}

#[derive(Debug, Copy, Clone)]
pub struct Trim<A: Alignment>(PhantomData<A>);
impl<A: Alignment> BlockOperation for Trim<A> {
    const IS_IDEMPOTENT: bool = true;
    const MODIFIES_MEDIA: bool = true;
    type RequiredAccess = ReadOnly;
}

/// reference: ZNS (https://www.usenix.org/system/files/atc21-bjorling.pdf).
#[derive(Debug, Copy, Clone)]
pub struct ZoneAppend<A: Alignment>(PhantomData<A>);
impl<A: Alignment> BlockOperation for ZoneAppend<A> {
    const IS_IDEMPOTENT: bool = false;
    const MODIFIES_MEDIA: bool = true;
    type RequiredAccess = ReadOnly;
}
