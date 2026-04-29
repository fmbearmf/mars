#![no_std]
extern crate proc_macro;
use proc_macro::TokenStream;

macro_rules! noop_derive {
    ($($name:ident),*) => {
        $(
            #[proc_macro_derive($name, attributes(zerocopy))]
            #[allow(non_snake_case)]
            pub fn $name(_i: TokenStream) -> TokenStream {
                TokenStream::new()
            }
        )*
    };
}

noop_derive!(FromBytes, Unaligned, Immutable, KnownLayout);
