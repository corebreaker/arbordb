//! Procedural macros for ArborDb — the `#[derive(AData)]` macro.
//!
//! Do not depend on this crate directly; it is re-exported by `arbordb` behind
//! the `derive` feature. Generated code is fully `::arbordb::`-qualified.

mod accessors;
mod adata;
mod attr;
mod convert;
mod desc;
mod enums;
mod fields;
mod generics;

use crate::attr::ContainerAttrs;
use crate::generics::Generics;

use proc_macro::TokenStream;
use quote::format_ident;
use syn::{parse_macro_input, Data, DeriveInput};

/// Derives [`AData`](../arbordb/data/trait.AData.html) for a struct or enum,
/// generating its `ArborXxx` / `ArborXxxMut` accessors and an `ArborXxxDesc`
/// companion.
#[proc_macro_derive(AData, attributes(arbor))]
pub fn derive_adata(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    expand(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Dispatches on the input's shape (delegated conversion, struct, or enum; unions
/// rejected). Generics are supported via [`Generics`].
fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let container = ContainerAttrs::parse(&input.attrs)?;
    let generics = Generics::analyze(&input.generics, container.bound());

    // `from`/`into`/`try_from` store the value AS a target type, bypassing shredding.
    if container.delegates() {
        return convert::convert_impl(input, &container, &generics);
    }

    match &input.data {
        Data::Struct(_) => expand_struct(input, &container, &generics),
        Data::Enum(data) => enums::expand_enum(input, data, &container, &generics),
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(AData)] does not support unions",
        )),
    }
}

/// Expands `#[derive(AData)]` for a struct into its impl, accessors, and descriptor.
fn expand_struct(
    input: &DeriveInput,
    container: &ContainerAttrs,
    generics: &Generics,
) -> syn::Result<proc_macro2::TokenStream> {
    let fields = fields::named_fields(input, container)?;

    let name = &input.ident;
    let vis = &input.vis;
    let ref_name = format_ident!("Arbor{}", name);
    let mut_name = format_ident!("Arbor{}Mut", name);
    let desc_name = format_ident!("Arbor{}Desc", name);
    let field_names: Vec<String> = fields
        .iter()
        .filter(|field| field.attrs.in_shape() && !field.attrs.is_flatten())
        .map(|field| field.name.clone())
        .collect();

    let adata = adata::adata_impl(name, &ref_name, &mut_name, &fields, generics);
    let accessors = accessors::accessors(vis, &ref_name, &mut_name, &fields, generics);
    let desc = desc::desc_struct(vis, &desc_name, &name.to_string(), &field_names);

    Ok(quote::quote! {
        #adata
        #accessors
        #desc
    })
}
