//! Extraction of a struct's named fields, with their `#[arbor(...)]` naming applied.

use crate::attr::{ContainerAttrs, FieldAttrs};

use proc_macro2::Span;
use syn::{Data, DeriveInput, Fields, Ident, Type};

/// One named field of the derived struct.
pub(crate) struct Field<'a> {
    /// The field identifier (also the read getter name).
    pub(crate) ident:   &'a Ident,
    /// The field type.
    pub(crate) ty:      &'a Type,
    /// The stored node name: an explicit `rename`, else the container's
    /// `rename_all` applied to the identifier, else the identifier verbatim.
    pub(crate) name:    String,
    /// Extra names accepted on load (load-only; the stored name stays primary).
    pub(crate) aliases: Vec<String>,
}

/// Extracts the named fields of a struct, rejecting the shapes not yet supported
/// and resolving each field's stored name from its `#[arbor(...)]` attributes.
pub(crate) fn named_fields(input: &DeriveInput) -> syn::Result<Vec<Field<'_>>> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[derive(AData)] expected a struct here",
        ));
    };

    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[derive(AData)] supports only structs with named fields",
        ));
    };

    let container = ContainerAttrs::parse(&input.attrs)?;

    named
        .named
        .iter()
        .map(|field| {
            let ident = field.ident.as_ref().expect("a named field has an identifier");
            let attrs = FieldAttrs::parse(&field.attrs)?;

            let name = attrs
                .rename
                .unwrap_or_else(|| container.rename_all.apply_to_field(&ident.to_string()));

            Ok(Field {
                ident,
                ty: &field.ty,
                name,
                aliases: attrs.aliases,
            })
        })
        .collect()
}
