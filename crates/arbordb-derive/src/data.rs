//! Codegen for the struct's `AData` impl (store / load).

use crate::{fields::Field, generics::Generics};
use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

/// Generates `impl AData for Struct`, storing/loading one child vnode per field.
pub(crate) fn a_data_impl(
    name: &Ident,
    ref_name: &Ident,
    mut_name: &Ident,
    fields: &[Field],
    generics: &Generics,
) -> TokenStream {
    // Only in-shape fields are written; `skip_store_if` makes the write conditional,
    // and `store_with` / `with` replace the field type's `AData::store`.
    let stores = fields.iter().filter(|field| field.attrs().in_shape()).map(|field| {
        let ident = field.ident();

        // Flattened: store the value at the parent's vnode, merging its fields in.
        if field.attrs().is_flatten() {
            return quote! {
                ::arbordb::data::AData::store(&self.#ident, writer, at)?;
            };
        }

        let stored = field.name();

        let store = match field.attrs().store_fn() {
            Some(path) => quote! {
                #path(&self.#ident, writer, &at.child_name(#stored))?;
            },
            None => quote! {
                ::arbordb::data::AData::store(&self.#ident, writer, &at.child_name(#stored))?;
            },
        };

        match &field.attrs().skip_store_if() {
            Some(predicate) => quote! {
                if !#predicate(&self.#ident) {
                    #store
                }
            },
            None => store,
        }
    });

    let loads = fields.iter().map(|field| {
        let ident = field.ident();
        let ty = field.ty();
        let stored = field.name();

        // Out of shape or `skip_load`: never read the vnode — produce the default.
        if !field.attrs().loads_from_node() {
            let default = field.attrs().default_expr();

            return quote! {
                #ident: #default,
            };
        }

        // Flattened: the value occupies the parent's vnode, so load from `at` directly.
        if field.attrs().is_flatten() {
            return quote! {
                #ident: <#ty as ::arbordb::data::AData>::load(reader, at)?,
            };
        }

        // `load_with` / `with` replace the field type's `AData::load`.
        let load_fn = field.attrs().load_fn();
        let load_from = |name: TokenStream| match &load_fn {
            Some(path) => quote! { #path(reader, &#name)? },
            None => quote! { <#ty as ::arbordb::data::AData>::load(reader, &#name)? },
        };

        // Fast path: no aliases and no default — load straight from the stored name.
        if field.attrs().aliases().is_empty() && field.attrs().field_default().is_none() {
            let load = load_from(quote! { at.child_name(#stored) });

            return quote! {
                #ident: #load,
            };
        }

        // Resolve the primary name, then each alias in order; on none present fall
        // back to the default when set, else to a direct (erroring) primary load.
        let aliases = &field.attrs().aliases();
        let chosen_load = load_from(quote! { at.child_name(__candidate) });
        let fallback = match field.attrs().field_default() {
            Some(_) => field.attrs().default_expr(),
            None => load_from(quote! { at.child_name(#stored) }),
        };

        quote! {
            #ident: {
                let mut __chosen: ::core::option::Option<&str> = ::core::option::Option::None;
                for __candidate in [#stored, #(#aliases),*] {
                    if ::arbordb::access::Reader::exists_at(reader, &at.child_name(__candidate))? {
                        __chosen = ::core::option::Option::Some(__candidate);

                        break;
                    }
                }

                match __chosen {
                    ::core::option::Option::Some(__candidate) => #chosen_load,
                    ::core::option::Option::None => #fallback,
                }
            },
        }
    });

    // A direct `encode_node` override (the typed direct encoder, bypassing the `Value`
    // tree) is possible when every stored field emits straight through
    // `AData::encode_node`: none is flattened (which merges another value's fields into
    // this object, needing a runtime merge) nor uses a custom `store_with` (which is a
    // `Writer`-based store, not a blob emission). Those cases keep the trait's
    // `Value`-based default, which is always correct — just not the fast path.
    let encode_node = {
        let storable: Vec<&Field> = fields.iter().filter(|field| field.attrs().in_shape()).collect();
        let simple = storable
            .iter()
            .all(|field| !field.attrs().is_flatten() && field.attrs().store_fn().is_none());

        if simple {
            // Emit the fields in stored-name order (the order a `BTreeMap` — and so the
            // `Value` default — would), children first, then the object vnode. A
            // `skip_store_if` field is pushed conditionally; the remaining entries stay
            // sorted, which `object` requires.
            let mut sorted = storable;
            sorted.sort_by(|a, b| a.name().cmp(b.name()));

            let count = sorted.len();
            let pushes = sorted.iter().map(|field| {
                let ident = field.ident();
                let stored = field.name();

                let emit = quote! {
                    let __off = ::arbordb::data::AData::encode_node(&self.#ident, __enc)?;
                    __entries.push((#stored, __off));
                };

                match field.attrs().skip_store_if() {
                    Some(predicate) => quote! {
                        if !#predicate(&self.#ident) {
                            #emit
                        }
                    },
                    None => emit,
                }
            });

            quote! {
                fn encode_node(
                    &self,
                    __enc: &mut ::arbordb::data::NodeEncoder,
                ) -> ::arbordb::AdbResult<u32> {
                    let mut __entries: ::std::vec::Vec<(&str, u32)> =
                        ::std::vec::Vec::with_capacity(#count);
                    #(#pushes)*

                    ::core::result::Result::Ok(__enc.object(&__entries))
                }
            }
        } else {
            quote! {}
        }
    };

    let impl_generics = generics.adata_impl();
    let ty_generics = generics.adata_ty();
    let where_clause = generics.adata_where();
    let accessor_ty = generics.accessor_ty();

    quote! {
        #[automatically_derived]
        impl #impl_generics ::arbordb::data::AData for #name #ty_generics #where_clause {
            type Ref<'t> = #ref_name #accessor_ty;
            type Mut<'t> = #mut_name #accessor_ty;

            fn store<__W: ::arbordb::access::Writer>(
                &self,
                writer: &__W,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<()> {
                ::arbordb::access::Writer::ensure_container(writer, at, false)?;
                #(#stores)*

                ::core::result::Result::Ok(())
            }

            fn load<__R: ::arbordb::access::Reader>(
                reader: &__R,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<Self> {
                ::core::result::Result::Ok(Self {
                    #(#loads)*
                })
            }

            #encode_node
        }
    }
}
