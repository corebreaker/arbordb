//! Codegen for the `AIndexed` impl from the `#[arbor(index(...))]` attributes.

use crate::{fields::Field, generics::Generics, index::IndexAttr};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Ident, Result as SynResult};

/// Generates `impl AIndexed for #name`, turning each `index(...)` declaration into
/// an `IndexDef` scoped to the caller-supplied `pattern`. A struct with no index
/// declarations still gets an impl returning an empty list.
pub(crate) fn indexed_impl(
    name: &Ident,
    fields: &[Field],
    indexes: &[IndexAttr],
    generics: &Generics,
) -> SynResult<TokenStream> {
    // Every column must name a field of the struct.
    for index in indexes {
        for column in index.columns() {
            if !fields.iter().any(|field| field.ident() == column.field()) {
                return Err(Error::new(
                    column.field().span(),
                    format!("index column `{col}` is not a field of `{name}`", col = column.field()),
                ));
            }
        }
    }

    let defs = indexes.iter().map(|index| {
        let index_name = index.name();
        let unique = index.unique();
        let columns = index.columns().iter().map(|column| {
            // The column path uses the field's STORED node name (rename-aware).
            let stored = fields
                .iter()
                .find(|field| field.ident() == column.field())
                .expect("column validated above")
                .name()
                .to_string();

            let path = quote! { ::arbordb::path::VPath::root().child_name(#stored) };

            if column.descending() {
                quote! { ::arbordb::index::IndexColumn::desc(#path) }
            } else {
                quote! { ::arbordb::index::IndexColumn::asc(#path) }
            }
        });

        quote! {
            ::arbordb::index::IndexDef::new(
                ::std::string::String::from(#index_name),
                ::std::string::String::from(pattern),
                ::std::vec![ #(#columns),* ],
                #unique,
            )
        }
    });

    let impl_generics = generics.adata_impl();
    let ty_generics = generics.adata_ty();
    let where_clause = generics.adata_where();

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::arbordb::index::AIndexed for #name #ty_generics #where_clause {
            fn index_defs(pattern: &str) -> ::std::vec::Vec<::arbordb::index::IndexDef> {
                ::std::vec![ #(#defs),* ]
            }
        }
    })
}
