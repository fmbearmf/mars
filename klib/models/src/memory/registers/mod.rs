//! it's like Tock registers, but without the heretical interior mutability

pub mod field;
pub mod volatile;

//pub use self::field::*;
//pub use self::volatile::*;

#[macro_export]
macro_rules! declare_register {
    ($(#[$meta:meta])*
        $reg_name:ident, $type:ty, {
        $(
            $(#[$f_meta:meta])*
            field $field_name:ident => (
                offset: $offset:expr,
                size: $width:expr
                $(, type: $field_type:ty)?
                $(, enum $enum_name:ident { $($variant:ident = $val:expr),+ $(,)? })?
                $(,)?
            );
        )*
    }) => {
        $(#[$meta])*
        pub struct $reg_name;

        $($(
            #[derive(Copy, Clone, PartialEq, Eq)]
            pub enum $enum_name {
                $($variant,)+
                Unknown($type),
            }
            impl $crate::memory::registers::field::FieldType<$type> for $enum_name {
                #[inline]
                fn from_bits(bits: $type) -> Self {
                    match bits {
                        $($val => Self::$variant,)+
                        _ => Self::Unknown(bits)
                    }
                }

                #[inline]
                fn into_bits(self) -> $type {
                    match self {
                        $(Self::$variant => $val,)+
                        Self::Unknown(bits) => bits
                    }
                }
            }
        )?)*

        impl $reg_name {
            $(
                $(#[$f_meta])*
                #[allow(non_upper_case_globals)]
                pub const $field_name: $crate::memory::registers::field::Field<
                    $reg_name,
                    $offset,
                    $width,
                    $type,
                    $crate::__extract_field_type!($type $(, $field_type)? $(, $enum_name)?)
                > = $crate::memory::registers::field::Field::new();
            )*
        }

        const _: () = {
            let mut total_mask: u128 = 0;
            let max_bits = core::mem::size_of::<$type>() as u8 * 8;

            $(
                if $offset + $width > max_bits {
                    panic!(concat!("field `", stringify!($field_name), "` exceeds register width"));
                }

                let mask: u128 = if $width == 128 { !0 } else { ((1u128 << $width) - 1) << $offset };

                if (total_mask & mask) != 0 {
                    panic!(concat!("field `", stringify!($field_name), "` overlaps with another field"));
                }

                total_mask |= mask;
            )*
        };
    };
}

#[macro_export]
macro_rules! declare_structs {
    (
        $(#[$struct_meta:meta])*
        $vis:vis $name:ident {
            $($fields:tt)*
        }
    ) => {
        $crate::_declare_fields! {
            @tracker
            meta: [ $(#[$struct_meta])* ];
            vis: [ $vis ];
            name: [ $name ];
            fields_acc: [];
            assert_acc: [];
            curr_sorr: 0;
            input: [ $($fields)* ];
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! _declare_fields {
    // generate field with padding
    (
        @tracker
        meta: [ $(#[$m:meta])* ];
        vis: [ $v:vis ];
        name: [ $n:ident ];
        fields_acc: [ $($facc:tt)* ];
        assert_acc: [ $($aacc:tt)* ];
        curr_sorr: $curr_sorr:expr;
        input: [
            $(#[$f_m:meta])* ($offset:expr => $fvis:vis $fname:ident : $fty:ty),
            $($rest:tt)*
        ];
    ) => {
        $crate::paste! {
            $crate::_declare_fields! {
                @tracker
                meta: [ $(#[$m])* ];
                vis: [ $v ];
                name: [ $n ];
                fields_acc: [
                    $($facc)*
                    [< _reserved_ $fname >]: [u8; $offset - $curr_sorr],
                    $(#[$f_m])* $fvis $fname: $fty,
                ];
                assert_acc: [
                    $($aacc)*
                    const _: () = assert!($offset >= $curr_sorr, "overlapping offsets or out of order");
                ];
                curr_sorr: ($offset + core::mem::size_of::<$fty>());
                input: [
                    $($rest)*
                ];
            }
        }
    };

    (
        @tracker
        meta: [ $(#[$m:meta])* ];
        vis: [ $v:vis ];
        name: [ $n:ident ];
        fields_acc: [ $($facc:tt)* ];
        assert_acc: [ $($aacc:tt)* ];
        curr_sorr: $curr_sorr:expr;
        input: [ ($end_offset:expr => @END) $(,)? ];
    ) => {
        $crate::paste! {
            $(#[$m])*
            #[repr(C)]
            $v struct $n {
                $($facc)*
                [< _reserved_end_ $n >]: [u8; $end_offset - $curr_sorr],
            }

            const _: () = {
                $($aacc)*
                assert!($end_offset >= $curr_sorr, "end offset < last field end");
            };
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __extract_field_type {
    ($reg:ty, $t:ty) => {
        $t
    };
    ($reg:ty, $t:ty, $e:ident) => {
        $e
    };
    ($reg:ty, , $e:ident) => {
        $e
    };
    ($reg:ty) => {
        $reg
    };
}

#[macro_export]
macro_rules! to_mask {
    ($num:expr) => {
        (1 << $num) - 1
    };
}
