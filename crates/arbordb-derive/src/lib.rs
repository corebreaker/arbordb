//! Procedural macros for ArborDb — the `#[derive(AData)]` macro.
//!
//! Do not depend on this crate directly; it is re-exported by `arbordb` behind
//! the `derive` feature. Generated code is fully `::arbordb::`-qualified.

mod accessors;
mod attr;
mod convert;
mod data;
mod desc;
mod enums;
mod expand;
mod fields;
mod generics;
mod index;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

/// Derives [`AData`](../arbordb/data/trait.AData.html) for a struct or enum,
/// generating its `ArborXxx` / `ArborXxxMut` accessors and an `ArborXxxDesc`
/// companion.
#[proc_macro_derive(AData, attributes(arbor))]
pub fn derive_adata(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand::expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
