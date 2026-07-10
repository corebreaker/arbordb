//! Codegen for the `ArborXxxDesc` companion: compile-time metadata about the type.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Visibility};

/// Generates a zero-size `ArborXxxDesc` for a struct: its type name and fields.
pub(crate) fn desc_struct(vis: &Visibility, desc_name: &Ident, type_name: &str, field_names: &[String]) -> TokenStream {
    quote! {
        #[allow(dead_code)]
        #vis struct #desc_name;

        impl #desc_name {
            /// The Rust type name.
            #vis const TYPE_NAME: &'static str = #type_name;

            /// The stored field names, in declaration order.
            #vis const FIELDS: &'static [&'static str] = &[#(#field_names),*];
        }
    }
}

/// Generates a zero-size `ArborXxxDesc` for an enum: its type name and variants.
pub(crate) fn desc_enum(vis: &Visibility, desc_name: &Ident, type_name: &str, variant_names: &[String]) -> TokenStream {
    quote! {
        #[allow(dead_code)]
        #vis struct #desc_name;

        impl #desc_name {
            /// The Rust type name.
            #vis const TYPE_NAME: &'static str = #type_name;

            /// The variant tag names, in declaration order.
            #vis const VARIANTS: &'static [&'static str] = &[#(#variant_names),*];
        }
    }
}
