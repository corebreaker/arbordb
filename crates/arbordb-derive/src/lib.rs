//! Procedural macros for ArborDb — the `#[derive(AData)]` macro.
//!
//! Do not depend on this crate directly; it is re-exported by `arbordb` behind
//! the `derive` feature. Generated code is fully `::arbordb::`-qualified.

mod accessors;
mod adata;
mod desc;
mod fields;

use proc_macro::TokenStream;
use quote::format_ident;
use syn::{parse_macro_input, DeriveInput};

/// Derives [`AData`](../arbordb/data/trait.AData.html) for a struct, generating
/// its `ArborXxx` / `ArborXxxMut` accessors and an `ArborXxxDesc` companion.
#[proc_macro_derive(AData, attributes(arbor))]
pub fn derive_adata(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Expands `#[derive(AData)]` for a struct into its impl, accessors, and descriptor.
fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "#[derive(AData)] does not support generics yet",
        ));
    }

    let fields = fields::named_fields(input)?;

    let name = &input.ident;
    let vis = &input.vis;
    let ref_name = format_ident!("Arbor{}", name);
    let mut_name = format_ident!("Arbor{}Mut", name);
    let desc_name = format_ident!("Arbor{}Desc", name);
    let field_names: Vec<String> = fields.iter().map(|field| field.name.clone()).collect();

    let adata = adata::adata_impl(name, &ref_name, &mut_name, &fields);
    let accessors = accessors::accessors(vis, &ref_name, &mut_name, &fields);
    let desc = desc::desc(vis, &desc_name, &name.to_string(), &field_names);

    Ok(quote::quote! {
        #adata
        #accessors
        #desc
    })
}
