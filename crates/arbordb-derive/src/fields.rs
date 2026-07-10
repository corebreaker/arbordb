//! Extraction of a struct's named fields.

use proc_macro2::Span;
use syn::{Data, DeriveInput, Fields, Ident, Type};

/// One named field of the derived struct.
pub(crate) struct Field<'a> {
    /// The field identifier (also the read getter name).
    pub(crate) ident: &'a Ident,
    /// The field type.
    pub(crate) ty:    &'a Type,
    /// The stored node name (the field name; renaming lands with attributes).
    pub(crate) name:  String,
}

/// Extracts the named fields of a struct, rejecting the shapes not yet supported.
pub(crate) fn named_fields(input: &DeriveInput) -> syn::Result<Vec<Field<'_>>> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[derive(AData)] currently supports only structs (enums land next)",
        ));
    };

    let Fields::Named(named) = &data.fields else {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[derive(AData)] supports only structs with named fields",
        ));
    };

    let fields = named
        .named
        .iter()
        .map(|field| {
            let ident = field.ident.as_ref().expect("a named field has an identifier");

            Field {
                ident,
                ty: &field.ty,
                name: ident.to_string(),
            }
        })
        .collect();

    Ok(fields)
}
