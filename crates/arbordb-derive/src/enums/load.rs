//! Load codegen for enum variants, per representation.

use crate::enums::VariantInfo;
use crate::enums::repr::EnumRepr;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Fields, Ident};

/// The `match tag` load arm for one variant (External / Adjacent; Internal is
/// dispatched to [`internal_load_arm`]). The arm matches the primary tag or any
/// alias.
pub(super) fn load_arm(info: &VariantInfo, repr: &EnumRepr) -> TokenStream {
    if let EnumRepr::Internal {
        ..
    } = repr
    {
        return internal_load_arm(info);
    }

    let id = &info.variant.ident;
    let tag = &info.tag;
    let aliases = &info.aliases;
    let base = repr.payload_base_load();

    match &info.variant.fields {
        Fields::Unit => quote! {
            #tag #(| #aliases)* => ::core::result::Result::Ok(Self::#id),
        },
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let ty = &fields.unnamed[0].ty;

            quote! {
                #tag #(| #aliases)* => ::core::result::Result::Ok(Self::#id(
                    <#ty as ::arbordb::data::AData>::load(reader, &#base)?,
                )),
            }
        }
        Fields::Unnamed(fields) => {
            let loads = fields.unnamed.iter().enumerate().map(|(index, field)| {
                let ty = &field.ty;
                let index = index as u64;

                quote! {
                    <#ty as ::arbordb::data::AData>::load(reader, &payload.child_index(#index))?
                }
            });

            quote! {
                #tag #(| #aliases)* => {
                    let payload = #base;

                    ::core::result::Result::Ok(Self::#id(#(#loads),*))
                }
            }
        }
        Fields::Named(fields) => {
            let inits = fields.named.iter().map(|field| {
                let name = field.ident.as_ref().unwrap();
                let name_str = name.to_string();
                let ty = &field.ty;

                quote! {
                    #name: <#ty as ::arbordb::data::AData>::load(reader, &payload.child_name(#name_str))?
                }
            });

            quote! {
                #tag #(| #aliases)* => {
                    let payload = #base;

                    ::core::result::Result::Ok(Self::#id { #(#inits),* })
                }
            }
        }
    }
}

/// Internal tagging: rebuild from the flattened object at `at`; tuple/newtype
/// elements are keyed by their decimal index.
fn internal_load_arm(info: &VariantInfo) -> TokenStream {
    let id = &info.variant.ident;
    let tag = &info.tag;
    let aliases = &info.aliases;

    match &info.variant.fields {
        Fields::Unit => quote! {
            #tag #(| #aliases)* => ::core::result::Result::Ok(Self::#id),
        },
        Fields::Unnamed(fields) => {
            let loads = fields.unnamed.iter().enumerate().map(|(index, field)| {
                let ty = &field.ty;
                let key = index.to_string();

                quote! {
                    <#ty as ::arbordb::data::AData>::load(reader, &at.child_name(#key))?
                }
            });

            quote! {
                #tag #(| #aliases)* => ::core::result::Result::Ok(Self::#id(#(#loads),*)),
            }
        }
        Fields::Named(fields) => {
            let inits = fields.named.iter().map(|field| {
                let name = field.ident.as_ref().unwrap();
                let name_str = name.to_string();
                let ty = &field.ty;

                quote! {
                    #name: <#ty as ::arbordb::data::AData>::load(reader, &at.child_name(#name_str))?
                }
            });

            quote! {
                #tag #(| #aliases)* => ::core::result::Result::Ok(Self::#id { #(#inits),* }),
            }
        }
    }
}

/// Untagged: one attempt block per variant, tried in declaration order; the first
/// that decodes wins.
pub(super) fn untagged_arm(info: &VariantInfo) -> TokenStream {
    let id = &info.variant.ident;

    match &info.variant.fields {
        Fields::Unit => quote! {
            if let ::core::result::Result::Ok(::core::option::Option::Some(::arbordb::data::Scalar::Null)) =
                ::arbordb::access::Reader::scalar_at(reader, at)
            {
                return ::core::result::Result::Ok(Self::#id);
            }
        },
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let ty = &fields.unnamed[0].ty;

            quote! {
                if let ::core::result::Result::Ok(inner) = <#ty as ::arbordb::data::AData>::load(reader, at) {
                    return ::core::result::Result::Ok(Self::#id(inner));
                }
            }
        }
        Fields::Unnamed(fields) => {
            let binds: Vec<Ident> = (0..fields.unnamed.len()).map(|i| format_ident!("f{i}")).collect();
            let lets = fields.unnamed.iter().enumerate().map(|(index, field)| {
                let ty = &field.ty;
                let bind = &binds[index];
                let index = index as u64;

                quote! {
                    let ::core::result::Result::Ok(#bind) =
                        <#ty as ::arbordb::data::AData>::load(reader, &at.child_index(#index))
                    else {
                        break '__attempt;
                    };
                }
            });

            quote! {
                '__attempt: {
                    #(#lets)*

                    return ::core::result::Result::Ok(Self::#id(#(#binds),*));
                }
            }
        }
        Fields::Named(fields) => {
            let names: Vec<&Ident> = fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            let lets = fields.named.iter().map(|field| {
                let name = field.ident.as_ref().unwrap();
                let name_str = name.to_string();
                let ty = &field.ty;

                quote! {
                    let ::core::result::Result::Ok(#name) =
                        <#ty as ::arbordb::data::AData>::load(reader, &at.child_name(#name_str))
                    else {
                        break '__attempt;
                    };
                }
            });

            quote! {
                '__attempt: {
                    #(#lets)*

                    return ::core::result::Result::Ok(Self::#id { #(#names),* });
                }
            }
        }
    }
}
