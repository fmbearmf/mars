use hax_lib::opaque;

use super::Unaligned;
use core::{convert::Infallible, marker::PhantomData};

pub enum ConvertError<A, S, V> {
    Alignment(A),
    Size(S),
    Validity(V),
}

pub struct SizeError<Src, Dst: ?Sized> {
    _phantom: PhantomData<Src>,
    _phantom2: PhantomData<Dst>,
}

pub struct AlignmentError<Src, Dst: ?Sized> {
    _phantom: PhantomData<Src>,
    _phantom2: PhantomData<Dst>,
}

pub type CastError<Src, Dst: ?Sized> =
    ConvertError<AlignmentError<Src, Dst>, SizeError<Src, Dst>, Infallible>;

impl<Src, Dst: ?Sized + Unaligned> From<CastError<Src, Dst>> for SizeError<Src, Dst> {
    fn from(value: CastError<Src, Dst>) -> Self {
        match value {
            CastError::Alignment(err) => match Infallible::from(err) {},
            CastError::Size(e) => e,
            CastError::Validity(i) => match i {},
        }
    }
}

#[opaque]
impl<Src, Dst: ?Sized + Unaligned> From<AlignmentError<Src, Dst>> for Infallible {
    #[inline(always)]
    fn from(_: AlignmentError<Src, Dst>) -> Self {
        // zerocopy only generates alignment errors when alignment > 1
        // therefore dst being unaligned makes that impossible
        unsafe { core::hint::unreachable_unchecked() }
    }
}
