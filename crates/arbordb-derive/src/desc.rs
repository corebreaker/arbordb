//! Codegen for the `ArborXxxDesc` companion: compile-time metadata about the type.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Visibility};

/// Generates a zero-size `ArborXxxDesc` with the type name and field names.
pub(crate) fn desc(vis: &Visibility, desc_name: &Ident, type_name: &str, field_names: &[String]) -> TokenStream {
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
