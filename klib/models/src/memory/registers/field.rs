use core::marker::PhantomData;
use core::ops::{Add, BitAnd, BitOr, Not, Shl, Shr, Sub};

use hax_lib::{exclude, opaque};

#[opaque]
pub trait RegisterValue:
    Sized
    + Copy
    + Not<Output = Self>
    + BitAnd<Output = Self>
    + BitOr<Output = Self>
    + Sub<Output = Self>
    + Add<Output = Self>
    + Shl<u8, Output = Self>
    + Shr<u8, Output = Self>
    + Eq
    + From<u8>
{
    const BITS: u8;
    const ZERO: Self;
    const ONE: Self;
    const ONES: Self;
}

macro_rules! impl_reg_value {
    ($($t:ty),*) => {
        $(
            impl RegisterValue for $t {
                const BITS: u8 = core::mem::size_of::<$t>() as u8 * 8;
                const ZERO: Self = 0;
                const ONE: Self = 1;
                const ONES: Self = !0;
            }
        )*
    };
}

impl_reg_value!(u8, u16, u32, u64, u128);

pub trait FieldType<T: RegisterValue>: Sized {
    fn from_bits(bits: T) -> Self;
    fn into_bits(self) -> T;
}

impl<T: RegisterValue> FieldType<T> for T {
    fn from_bits(bits: T) -> Self {
        bits
    }
    fn into_bits(self) -> T {
        self
    }
}

impl<T: RegisterValue> FieldType<T> for bool {
    fn from_bits(bits: T) -> Self {
        bits != T::ZERO
    }
    fn into_bits(self) -> T {
        if self { T::from(1) } else { T::ZERO }
    }
}

pub struct Field<Tag, const OFFSET: u8, const WIDTH: u8, T: RegisterValue, V: FieldType<T> = T> {
    _phantom: PhantomData<(Tag, T, V)>,
}

impl<Tag, const OFFSET: u8, const WIDTH: u8, T: RegisterValue, V: FieldType<T>>
    Field<Tag, OFFSET, WIDTH, T, V>
{
    pub const fn new() -> Self {
        assert!(
            OFFSET + WIDTH <= T::BITS,
            "field larger than register width"
        );
        Self {
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub fn mask(&self) -> T {
        let mask = if WIDTH == T::BITS {
            T::ONES
        } else {
            (T::ONES) >> (T::BITS - WIDTH)
        };
        mask << OFFSET
    }

    #[inline]
    pub fn lift(&self, raw: T) -> V {
        let mask = if WIDTH == T::BITS {
            T::ONES
        } else {
            (T::ONES) >> (T::BITS - WIDTH)
        };
        V::from_bits((raw >> OFFSET) & mask)
    }

    #[inline]
    pub fn lower(&self, value: V) -> T {
        let mask = if WIDTH == T::BITS {
            T::ONES
        } else {
            (T::ONES) >> (T::BITS - WIDTH)
        };
        (value.into_bits() & mask) << OFFSET
    }
}
