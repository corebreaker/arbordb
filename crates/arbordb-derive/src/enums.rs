//! Codegen for `#[derive(AData)]` on enums.
//!
//! Externally tagged (the default; the adjacent / internal / untagged
//! representations arrive with the `#[arbor(...)]` attributes): the value node is
//! an object with ONE field named after the active variant, holding the payload —
//! a unit variant's payload is a `Null` leaf, a newtype's is the inner value, a
//! tuple's is a list, a struct-variant's is an object. `store` clears the node
//! first, so exactly one tag survives a variant change.

use crate::desc;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{DataEnum, DeriveInput, Fields, Ident, Variant};

/// Expands `#[derive(AData)]` for an enum into its impl, accessors, and descriptor.
pub(crate) fn expand_enum(input: &DeriveInput, data: &DataEnum) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let vis = &input.vis;
    let ref_name = format_ident!("Arbor{}", name);
    let mut_name = format_ident!("Arbor{}Mut", name);
    let desc_name = format_ident!("Arbor{}Desc", name);

    let store_arms = data.variants.iter().map(|variant| store_arm(name, variant));
    let load_arms = data.variants.iter().map(load_arm);
    let variant_names: Vec<String> = data.variants.iter().map(|variant| variant.ident.to_string()).collect();

    let adata = quote! {
        #[automatically_derived]
        impl ::arbordb::data::AData for #name {
            type Ref<'t> = #ref_name<'t>;
            type Mut<'t> = #mut_name<'t>;

            fn store<__W: ::arbordb::access::Writer>(
                &self,
                writer: &__W,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<()> {
                ::arbordb::access::Writer::remove(writer, at)?;
                ::arbordb::access::Writer::ensure_container(writer, at, false)?;

                match self {
                    #(#store_arms)*
                }

                ::core::result::Result::Ok(())
            }

            fn load<__R: ::arbordb::access::Reader>(
                reader: &__R,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<Self> {
                let tag = ::arbordb::access::Reader::keys_at(reader, at)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| ::arbordb::AdbError::Corrupt("enum node has no variant tag".into()))?;

                match tag.as_str() {
                    #(#load_arms)*
                    other => ::core::result::Result::Err(::arbordb::AdbError::Corrupt(::std::format!(
                        "unknown enum variant tag: {other}"
                    ))),
                }
            }
        }
    };

    let accessors = accessors(input, &ref_name, &mut_name);
    let desc = desc::desc_enum(vis, &desc_name, &name.to_string(), &variant_names);

    Ok(quote! {
        #adata
        #accessors
        #desc
    })
}

/// The `match self` store arm for one variant (external tagging).
fn store_arm(name: &Ident, variant: &Variant) -> TokenStream {
    let ident = &variant.ident;
    let tag = ident.to_string();

    match &variant.fields {
        Fields::Unit => quote! {
            #name::#ident => {
                ::arbordb::access::Writer::put_scalar(writer, &at.child_name(#tag), ::arbordb::data::Scalar::Null)?;
            }
        },
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => quote! {
            #name::#ident(value) => {
                ::arbordb::data::AData::store(value, writer, &at.child_name(#tag))?;
            }
        },
        Fields::Unnamed(fields) => {
            let bindings: Vec<Ident> = (0..fields.unnamed.len()).map(|i| format_ident!("field{i}")).collect();
            let stores = bindings.iter().enumerate().map(|(index, binding)| {
                let index = index as u64;

                quote! {
                    ::arbordb::data::AData::store(#binding, writer, &payload.child_index(#index))?;
                }
            });

            quote! {
                #name::#ident(#(#bindings),*) => {
                    let payload = at.child_name(#tag);
                    ::arbordb::access::Writer::ensure_container(writer, &payload, true)?;
                    #(#stores)*
                }
            }
        }
        Fields::Named(fields) => {
            let idents: Vec<&Ident> = fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            let stores = idents.iter().map(|id| {
                let field_name = id.to_string();

                quote! {
                    ::arbordb::data::AData::store(#id, writer, &payload.child_name(#field_name))?;
                }
            });

            quote! {
                #name::#ident { #(#idents),* } => {
                    let payload = at.child_name(#tag);
                    ::arbordb::access::Writer::ensure_container(writer, &payload, false)?;
                    #(#stores)*
                }
            }
        }
    }
}

/// The `match tag` load arm for one variant (external tagging).
fn load_arm(variant: &Variant) -> TokenStream {
    let ident = &variant.ident;
    let tag = ident.to_string();

    match &variant.fields {
        Fields::Unit => quote! {
            #tag => ::core::result::Result::Ok(Self::#ident),
        },
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let ty = &fields.unnamed[0].ty;

            quote! {
                #tag => ::core::result::Result::Ok(Self::#ident(
                    <#ty as ::arbordb::data::AData>::load(reader, &at.child_name(#tag))?,
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
                #tag => {
                    let payload = at.child_name(#tag);

                    ::core::result::Result::Ok(Self::#ident(#(#loads),*))
                }
            }
        }
        Fields::Named(fields) => {
            let loads = fields.named.iter().map(|field| {
                let id = field.ident.as_ref().unwrap();
                let ty = &field.ty;
                let field_name = id.to_string();

                quote! {
                    #id: <#ty as ::arbordb::data::AData>::load(reader, &payload.child_name(#field_name))?
                }
            });

            quote! {
                #tag => {
                    let payload = at.child_name(#tag);

                    ::core::result::Result::Ok(Self::#ident { #(#loads),* })
                }
            }
        }
    }
}

/// The minimal enum accessors: read and write cursors exposing only `variant()`
/// (the active tag). The full payload is recomposed with `load::<E>`.
fn accessors(input: &DeriveInput, ref_name: &Ident, mut_name: &Ident) -> TokenStream {
    let vis = &input.vis;

    quote! {
        #[allow(dead_code)]
        #vis struct #ref_name<'t> {
            reader: ::std::sync::Arc<dyn ::arbordb::access::Reader + 't>,
            base:   ::arbordb::path::VPath,
        }

        impl<'t> #ref_name<'t> {
            /// The active variant's tag name.
            #vis fn variant(&self) -> ::arbordb::AdbResult<::std::string::String> {
                ::arbordb::access::Reader::keys_at(&self.reader, &self.base)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| ::arbordb::AdbError::Corrupt("enum node has no variant tag".into()))
            }
        }

        impl<'t> ::arbordb::data::ARef<'t> for #ref_name<'t> {
            fn open(
                reader: ::std::sync::Arc<dyn ::arbordb::access::Reader + 't>,
                base: ::arbordb::path::VPath,
            ) -> Self {
                Self {
                    reader,
                    base,
                }
            }
        }

        impl<'t> ::arbordb::data::AIdentifiable for #ref_name<'t> {
            fn path(&self) -> &::arbordb::path::VPath {
                &self.base
            }
        }

        #[allow(dead_code)]
        #vis struct #mut_name<'t> {
            writer: ::std::sync::Arc<dyn ::arbordb::access::Writer + 't>,
            base:   ::arbordb::path::VPath,
        }

        impl<'t> #mut_name<'t> {
            /// The active variant's tag name.
            #vis fn variant(&self) -> ::arbordb::AdbResult<::std::string::String> {
                ::arbordb::access::Reader::keys_at(&self.writer, &self.base)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| ::arbordb::AdbError::Corrupt("enum node has no variant tag".into()))
            }
        }

        impl<'t> ::arbordb::data::AMut<'t> for #mut_name<'t> {
            fn open(
                writer: ::std::sync::Arc<dyn ::arbordb::access::Writer + 't>,
                base: ::arbordb::path::VPath,
            ) -> Self {
                Self {
                    writer,
                    base,
                }
            }
        }

        impl<'t> ::arbordb::data::AIdentifiable for #mut_name<'t> {
            fn path(&self) -> &::arbordb::path::VPath {
                &self.base
            }
        }
    }
}
