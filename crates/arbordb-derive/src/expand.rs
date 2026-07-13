//! Top-level `#[derive(AData)]` expansion: dispatch on the input's shape and
//! assemble a struct's `AData` impl, accessors, descriptor, and `AIndexed` impl.
//! Enums are handed off to [`enums`]; delegated conversions to [`convert`].

use crate::{attr::ContainerAttrs, generics::Generics, accessors, convert, data, desc, enums, fields, index};
use quote::format_ident;
use syn::{Data, DeriveInput, Result as SynResult};

/// Dispatches on the input's shape (delegated conversion, struct, or enum; unions
/// rejected). Generics are supported via [`Generics`].
pub(super) fn expand(input: &DeriveInput) -> SynResult<proc_macro2::TokenStream> {
    let container = ContainerAttrs::parse(&input.attrs)?;
    let generics = Generics::analyze(&input.generics, container.bound());

    // `from`/`into`/`try_from` store the value AS a target type, bypassing shredding.
    if container.delegates() {
        return convert::convert_impl(input, &container, &generics);
    }

    match &input.data {
        Data::Struct(_) => expand_struct(input, &container, &generics),
        Data::Enum(data) => {
            // Index columns name a struct's fields; an enum exposes none.
            if let Some(index) = container.indexes().first() {
                // no-coverage:start — attribute-validation error (invalid input never compiles)
                return Err(syn::Error::new_spanned(
                    index.name(),
                    "`#[arbor(index(...))]` is not supported on enums",
                ));
                // no-coverage:stop
            }

            enums::expand_enum(input, data, &container, &generics)
        }
        // no-coverage:start — a union never reaches the value model (invalid input never compiles)
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(AData)] does not support unions",
        )),
        // no-coverage:stop
    }
}

/// Expands `#[derive(AData)]` for a struct into its impl, accessors, and descriptor.
fn expand_struct(
    input: &DeriveInput,
    container: &ContainerAttrs,
    generics: &Generics,
) -> SynResult<proc_macro2::TokenStream> {
    let fields = fields::named_fields(input, container)?;

    let name = &input.ident;
    let vis = &input.vis;
    let ref_name = format_ident!("Arbor{}", name);
    let mut_name = format_ident!("Arbor{}Mut", name);
    let desc_name = format_ident!("Arbor{}Desc", name);
    let field_names: Vec<String> = fields
        .iter()
        .filter(|field| field.attrs().in_shape() && !field.attrs().is_flatten())
        .map(|field| field.name().to_string())
        .collect();

    let adata = data::a_data_impl(name, &ref_name, &mut_name, &fields, generics);
    let accessors = accessors::accessors(vis, &ref_name, &mut_name, &fields, generics);
    let desc = desc::desc_struct(vis, &desc_name, &name.to_string(), &field_names);
    let indexed = index::indexed_impl(name, &fields, container.indexes(), generics)?;

    Ok(quote::quote! {
        #adata
        #accessors
        #desc
        #indexed
    })
}
