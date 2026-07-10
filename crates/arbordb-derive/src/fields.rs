//! Extraction of a struct's named fields, with their `#[arbor(...)]` naming applied.

use crate::attr::{ContainerAttrs, FieldAttrs};
use proc_macro2::Span;
use syn::{Data, DeriveInput, Fields, Ident, Type, Result as SynResult};

/// One named field of the derived struct.
pub(crate) struct Field<'a> {
    /// The field identifier (also the read getter name).
    ident: &'a Ident,
    /// The field type.
    ty:    &'a Type,
    /// The stored vnode name: an explicit `rename`, else the container's
    /// `rename_all` applied to the identifier, else the identifier verbatim.
    name:  String,
    /// The field's parsed `#[arbor(...)]` attributes (aliases, skip family, default).
    attrs: FieldAttrs,
}

impl<'a> Field<'a> {
    /// The field identifier (also the read getter name).
    pub(crate) fn ident(&self) -> &'a Ident {
        self.ident
    }

    /// The field type.
    pub(crate) fn ty(&self) -> &'a Type {
        self.ty
    }

    /// The stored vnode name (rename-aware; see the struct docs).
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The field's parsed `#[arbor(...)]` attributes.
    pub(crate) fn attrs(&self) -> &FieldAttrs {
        &self.attrs
    }
}

/// Extracts the named fields of a struct, rejecting the shapes not yet supported
/// and resolving each field's stored name from its `#[arbor(...)]` attributes.
pub(crate) fn named_fields<'a>(input: &'a DeriveInput, container: &ContainerAttrs) -> SynResult<Vec<Field<'a>>> {
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

    named
        .named
        .iter()
        .map(|field| {
            let ident = field.ident.as_ref().expect("a named field has an identifier");
            let attrs = FieldAttrs::parse(&field.attrs)?;

            let name = attrs
                .rename()
                .map(str::to_string)
                .unwrap_or_else(|| container.rename_all.apply_to_field(&ident.to_string()));

            Ok(Field {
                ident,
                ty: &field.ty,
                name,
                attrs,
            })
        })
        .collect()
}
