use proc_macro::TokenStream;
use quote::quote;
use syn::{Fields, ItemStruct, parse_macro_input};

#[proc_macro_attribute]
pub fn unaligned_getters(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemStruct);
    let struct_item = &input;

    let struct_ident = &input.ident;
    let generics = &input.generics;
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    let named_fields = match &input.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return syn::Error::new_spanned(
                input,
                "`#[unaligned_getters]` attribute only works with named fields",
            )
            .to_compile_error()
            .into();
        }
    };

    let getters = named_fields.iter().map(|f| {
        let name = &f.ident;
        let type_ = &f.ty;
        quote! {
            #[inline(always)]
            pub fn #name(&self) -> #type_
            where
                #type_: Copy,
            {
                unsafe { core::ptr::read_unaligned(&raw const self.#name) }
            }
        }
    });

    let expanded = quote! {
        #struct_item

        impl #impl_generics #struct_ident #type_generics #where_clause {
            #(#getters)*
        }
    };

    expanded.into()
}
