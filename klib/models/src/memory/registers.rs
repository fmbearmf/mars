//! it's like Tock registers, but without the heretical interior mutability

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
