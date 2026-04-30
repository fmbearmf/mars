use core::marker::PhantomData;
use core::ops::{BitAnd, Shr};
use core::ptr;

use hax_lib::opaque;
use zerocopy::{FromBytes, Immutable, KnownLayout};

use crate::memory::registers::field::FieldType;
use crate::{
    memory::registers::field::{Field, RegisterValue},
    to_mask,
};

pub trait Register {
    type Output: RegisterValue + Copy;
    type Tag;
}

pub trait Readable: Register {
    fn read_impure(&mut self) -> Self::Output;

    fn read_field_impure<const O: u8, const W: u8, V: FieldType<Self::Output>>(
        &mut self,
        field: Field<Self::Tag, O, W, Self::Output, V>,
    ) -> V;

    fn builder_impure(&mut self) -> RegisterModifier<Self::Output, Self::Tag>;
}

pub trait PureReadable: Readable {
    fn read_pure(&self) -> Self::Output;

    fn read_field_pure<const O: u8, const W: u8, V: FieldType<Self::Output>>(
        &self,
        field: Field<Self::Tag, O, W, Self::Output, V>,
    ) -> V;

    fn builder_pure(&self) -> RegisterModifier<Self::Output, Self::Tag>;
}

pub trait Writeable: Register {
    fn write(&mut self, value: Self::Output);

    fn modify_field<const O: u8, const W: u8, V: FieldType<Self::Output>>(
        &mut self,
        field: Field<Self::Tag, O, W, Self::Output, V>,
        value: V,
    );
}

pub trait RBuilder {
    type Tag;
    type T: RegisterValue;

    fn set<const O: u8, const W: u8, V: FieldType<Self::T>>(
        self,
        field: Field<Self::Tag, O, W, Self::T, V>,
        value: V,
    ) -> Self;
    fn commit(self);
}

pub struct RegisterModifier<T: RegisterValue, Tag> {
    value: T,
    _phantom: PhantomData<Tag>,
}

impl<'a, T: RegisterValue, Tag> RegisterModifier<T, Tag> {
    pub fn new(val: T) -> Self {
        Self {
            value: val,
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub fn set<const O: u8, const W: u8, V: FieldType<T>>(
        self,
        field: Field<Tag, O, W, T, V>,
        value: V,
    ) -> Self {
        let mask = field.mask();
        let new = (self.value & !mask) | field.lower(value);
        Self { value: new, ..self }
    }

    pub fn finish(self) -> T {
        self.value
    }
}

#[repr(transparent)]
#[derive(FromBytes, Immutable, KnownLayout)]
pub struct Volatile<T: RegisterValue, Tag> {
    data: T,
    _phantom_tag: PhantomData<Tag>,
}

#[opaque]
impl<T: RegisterValue + Copy, Tag> Volatile<T, Tag> {
    #[inline]
    pub fn read(&self) -> T {
        unsafe { ptr::read_volatile(&self.data as *const T) }
    }
    #[inline]
    pub fn write(&mut self, value: T) {
        unsafe { ptr::write_volatile(&mut self.data as *mut T, value) }
    }

    #[inline]
    pub fn read_field<const O: u8, const W: u8, V: FieldType<T>>(
        &self,
        field: Field<Tag, O, W, T, V>,
    ) -> V {
        field.lift(self.read())
    }

    #[inline]
    pub fn modify_field<const O: u8, const W: u8, V: FieldType<T>>(
        &mut self,
        field: Field<Tag, O, W, T, V>,
        value: V,
    ) {
        let mask = field.mask();
        let old = self.read();
        let new = (old & !mask) | field.lower(value);

        self.write(new);
    }

    #[inline]
    pub fn builder(&self) -> RegisterModifier<T, Tag> {
        RegisterModifier {
            value: self.read(),
            _phantom: PhantomData,
        }
    }
}

/// read-only field that has side effects
#[repr(transparent)]
#[derive(FromBytes, Immutable, KnownLayout)]
pub struct RReadOnly<T: RegisterValue, Tag>(Volatile<T, Tag>);

mod r_ro {
    use super::*;
    impl<T: RegisterValue + Copy, Tag> Register for RReadOnly<T, Tag> {
        type Output = T;
        type Tag = Tag;
    }

    impl<T: RegisterValue + Copy, Tag> Readable for RReadOnly<T, Tag> {
        fn read_impure(&mut self) -> T {
            self.0.read()
        }

        fn read_field_impure<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Tag, O, W, T, V>,
        ) -> V {
            self.0.read_field(field)
        }

        fn builder_impure(&mut self) -> RegisterModifier<T, Self::Tag> {
            self.0.builder()
        }
    }
}

/// read-only field that doesn't have side effects
#[repr(transparent)]
#[derive(FromBytes, Immutable, KnownLayout)]
pub struct RPureReadOnly<T: RegisterValue, Tag>(Volatile<T, Tag>);

mod r_purero {
    use super::*;
    impl<T: RegisterValue + Copy, Tag> Register for RPureReadOnly<T, Tag> {
        type Output = T;
        type Tag = Tag;
    }

    impl<T: RegisterValue + Copy, Tag> Readable for RPureReadOnly<T, Tag> {
        fn read_impure(&mut self) -> Self::Output {
            self.0.read()
        }

        fn read_field_impure<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Tag, O, W, T, V>,
        ) -> V {
            self.0.read_field(field)
        }

        fn builder_impure(&mut self) -> RegisterModifier<T, Self::Tag> {
            self.0.builder()
        }
    }

    impl<T: RegisterValue + Copy, Tag> PureReadable for RPureReadOnly<T, Tag> {
        fn read_pure(&self) -> Self::Output {
            self.0.read()
        }

        fn read_field_pure<const O: u8, const W: u8, V: FieldType<T>>(
            &self,
            field: Field<Tag, O, W, T, V>,
        ) -> V {
            self.0.read_field(field)
        }

        fn builder_pure(&self) -> RegisterModifier<T, Self::Tag> {
            self.0.builder()
        }
    }
}

/// write-only field
#[repr(transparent)]
#[derive(FromBytes, Immutable, KnownLayout)]
pub struct RWriteOnly<T: RegisterValue, Tag>(Volatile<T, Tag>);

mod r_writeonly {
    use super::*;
    impl<T: RegisterValue + Copy, Tag> Register for RWriteOnly<T, Tag> {
        type Output = T;
        type Tag = Tag;
    }

    impl<T: RegisterValue + Copy, Tag> Writeable for RWriteOnly<T, Tag> {
        fn write(&mut self, value: T) {
            self.0.write(value)
        }
        fn modify_field<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Self::Tag, O, W, T, V>,
            value: V,
        ) {
            // can't read. just write.
            self.0.write(field.lower(value));
        }
    }
}

/// r&w field where reading doesn't cause side effects
#[repr(transparent)]
#[derive(FromBytes, Immutable, KnownLayout)]
pub struct RPureReadWrite<T: RegisterValue, Tag>(Volatile<T, Tag>);

mod r_purerw {
    use super::*;
    impl<T: RegisterValue + Copy, Tag> Register for RPureReadWrite<T, Tag> {
        type Output = T;
        type Tag = Tag;
    }

    impl<T: RegisterValue + Copy, Tag> Writeable for RPureReadWrite<T, Tag> {
        fn write(&mut self, value: Self::Output) {
            self.0.write(value)
        }
        fn modify_field<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Self::Tag, O, W, Self::Output, V>,
            value: V,
        ) {
            self.0.modify_field(field, value)
        }
    }

    impl<T: RegisterValue + Copy, Tag> Readable for RPureReadWrite<T, Tag> {
        fn read_impure(&mut self) -> T {
            self.0.read()
        }

        fn read_field_impure<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Tag, O, W, T, V>,
        ) -> V {
            self.0.read_field(field)
        }

        fn builder_impure(&mut self) -> RegisterModifier<T, Self::Tag> {
            self.0.builder()
        }
    }

    impl<T: RegisterValue + Copy, Tag> PureReadable for RPureReadWrite<T, Tag> {
        fn read_pure(&self) -> T {
            self.0.read()
        }

        fn read_field_pure<const O: u8, const W: u8, V: FieldType<T>>(
            &self,
            field: Field<Tag, O, W, T, V>,
        ) -> V {
            self.0.read_field(field)
        }

        fn builder_pure(&self) -> RegisterModifier<T, Self::Tag> {
            self.0.builder()
        }
    }
}

/// r&w field where reading causes side effects
#[repr(transparent)]
#[derive(FromBytes, Immutable, KnownLayout)]
pub struct RReadWrite<T: RegisterValue, Tag>(Volatile<T, Tag>);

mod r_rw {
    use super::*;
    impl<T: RegisterValue + Copy, Tag> Register for RReadWrite<T, Tag> {
        type Output = T;
        type Tag = Tag;
    }

    impl<T: RegisterValue + Copy, Tag> Writeable for RReadWrite<T, Tag> {
        fn write(&mut self, value: T) {
            self.0.write(value)
        }
        fn modify_field<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Self::Tag, O, W, T, V>,
            value: V,
        ) {
            self.0.modify_field(field, value)
        }
    }

    impl<T: RegisterValue + Copy, Tag> Readable for RReadWrite<T, Tag> {
        fn read_impure(&mut self) -> T {
            self.0.read()
        }

        fn read_field_impure<const O: u8, const W: u8, V: FieldType<T>>(
            &mut self,
            field: Field<Tag, O, W, T, V>,
        ) -> V {
            self.0.read_field(field)
        }

        fn builder_impure(&mut self) -> RegisterModifier<T, Self::Tag> {
            self.0.builder()
        }
    }
}
