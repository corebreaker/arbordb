//! Store codegen for enum variants, per representation.

use crate::enums::VariantInfo;
use crate::enums::repr::EnumRepr;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Fields, Ident};

/// The `match self` store arm for one variant (External / Adjacent / Untagged;
/// Internal is dispatched to [`internal_store_arm`]).
pub(super) fn store_arm(info: &VariantInfo, repr: &EnumRepr) -> TokenStream {
    if let EnumRepr::Internal {
        ..
    } = repr
    {
        return internal_store_arm(info, repr);
    }

    let id = &info.variant.ident;
    let tag = &info.tag;
    let tag_store = repr.tag_store(tag);
    let base = repr.payload_base_store(tag);

    match &info.variant.fields {
        Fields::Unit => {
            let unit_body = repr.unit_store(tag);

            quote! {
                Self::#id => {
                    #unit_body
                }
            }
        }
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => quote! {
            Self::#id(inner) => {
                #tag_store
                ::arbordb::data::AData::store(inner, writer, &#base)?;
            }
        },
        Fields::Unnamed(fields) => {
            let binds: Vec<Ident> = (0..fields.unnamed.len()).map(|i| format_ident!("f{i}")).collect();
            let stores = binds.iter().enumerate().map(|(index, bind)| {
                let index = index as u64;

                quote! {
                    ::arbordb::data::AData::store(#bind, writer, &payload.child_index(#index))?;
                }
            });

            quote! {
                Self::#id(#(#binds),*) => {
                    #tag_store
                    let payload = #base;
                    ::arbordb::access::Writer::ensure_container(writer, &payload, true)?;
                    #(#stores)*
                }
            }
        }
        Fields::Named(fields) => {
            let names: Vec<&Ident> = fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            let name_strs: Vec<String> = names.iter().map(|n| n.to_string()).collect();

            quote! {
                Self::#id { #(#names),* } => {
                    #tag_store
                    let payload = #base;
                    ::arbordb::access::Writer::ensure_container(writer, &payload, false)?;
                    #( ::arbordb::data::AData::store(#names, writer, &payload.child_name(#name_strs))?; )*
                }
            }
        }
    }
}

/// Internal tagging: the tag plus the payload flattened into a single object at
/// `at`; tuple/newtype elements are keyed by their decimal index.
fn internal_store_arm(info: &VariantInfo, repr: &EnumRepr) -> TokenStream {
    let id = &info.variant.ident;
    let tag_store = repr.tag_store(&info.tag);

    match &info.variant.fields {
        Fields::Unit => quote! {
            Self::#id => {
                #tag_store
            }
        },
        Fields::Unnamed(fields) => {
            let binds: Vec<Ident> = (0..fields.unnamed.len()).map(|i| format_ident!("f{i}")).collect();
            let stores = binds.iter().enumerate().map(|(index, bind)| {
                let key = index.to_string();

                quote! {
                    ::arbordb::data::AData::store(#bind, writer, &at.child_name(#key))?;
                }
            });

            quote! {
                Self::#id(#(#binds),*) => {
                    #tag_store
                    #(#stores)*
                }
            }
        }
        Fields::Named(fields) => {
            let names: Vec<&Ident> = fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            let name_strs: Vec<String> = names.iter().map(|n| n.to_string()).collect();

            quote! {
                Self::#id { #(#names),* } => {
                    #tag_store
                    #( ::arbordb::data::AData::store(#names, writer, &at.child_name(#name_strs))?; )*
                }
            }
        }
    }
}
