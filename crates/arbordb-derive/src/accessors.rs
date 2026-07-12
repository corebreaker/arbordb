//! Codegen for the read (`ArborXxx`) and write (`ArborXxxMut`) accessor types.
//!
//! An accessor pairs a shared cursor with a base `VPath`. A getter is
//! **infallible** — it just clones the cursor and extends the path (no I/O); the
//! read happens later on the returned accessor (`.get()` on a leaf, a nested
//! getter, …). This is the ArborDb difference from StratoDb: navigation is by
//! `VPath`, so no per-vnode key is resolved or stored.

use crate::{fields::Field, generics::Generics};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Visibility};

/// Generates both accessor types, their getters, and their trait impls.
pub(crate) fn accessors(
    vis: &Visibility,
    ref_name: &Ident,
    mut_name: &Ident,
    fields: &[Field],
    generics: &Generics,
) -> TokenStream {
    let ref_getters = fields.iter().filter(|field| field.attrs().in_shape()).map(|field| {
        let getter = field.ident();
        let ty = field.ty();

        // A flattened field shares the parent's vnode: open the accessor right there.
        let base = if field.attrs().is_flatten() {
            quote! { self.base.clone() }
        } else {
            let stored = field.name();

            quote! { self.base.child_name(#stored) }
        };

        quote! {
            #vis fn #getter(&self) -> <#ty as ::arbordb::data::AData>::Ref<'t> {
                <<#ty as ::arbordb::data::AData>::Ref<'t> as ::arbordb::data::ARef<'t>>::open(
                    ::std::sync::Arc::clone(&self.reader),
                    #base,
                )
            }
        }
    });

    let mut_getters = fields.iter().filter(|field| field.attrs().in_shape()).map(|field| {
        let getter = format_ident!("{}_mut", field.ident());
        let ty = field.ty();

        // A flattened field shares the parent's vnode: open the accessor right there.
        let base = if field.attrs().is_flatten() {
            quote! { self.base.clone() }
        } else {
            let stored = field.name();

            quote! { self.base.child_name(#stored) }
        };

        quote! {
            #vis fn #getter(&self) -> <#ty as ::arbordb::data::AData>::Mut<'t> {
                <<#ty as ::arbordb::data::AData>::Mut<'t> as ::arbordb::data::AMut<'t>>::open(
                    ::std::sync::Arc::clone(&self.writer),
                    #base,
                )
            }
        }
    });

    let accessor_impl = generics.accessor_impl();
    let accessor_ty = generics.accessor_ty();
    let accessor_where = generics.accessor_where();
    let phantom_field = generics.phantom_field();
    let phantom_init = generics.phantom_init();

    quote! {
        #[allow(dead_code)]
        #vis struct #ref_name #accessor_impl #accessor_where {
            reader: ::std::sync::Arc<dyn ::arbordb::access::Reader + 't>,
            base:   ::arbordb::path::VPath,
            #phantom_field
        }

        impl #accessor_impl #ref_name #accessor_ty #accessor_where {
            #(#ref_getters)*
        }

        impl #accessor_impl ::arbordb::data::ARef<'t> for #ref_name #accessor_ty #accessor_where {
            fn open(
                reader: ::std::sync::Arc<dyn ::arbordb::access::Reader + 't>,
                base: ::arbordb::path::VPath,
            ) -> Self {
                Self {
                    reader,
                    base,
                    #phantom_init
                }
            }
        }

        impl #accessor_impl ::arbordb::data::AIdentifiable for #ref_name #accessor_ty #accessor_where {
            fn path(&self) -> &::arbordb::path::VPath {
                &self.base
            }
        }

        #[allow(dead_code)]
        #vis struct #mut_name #accessor_impl #accessor_where {
            writer: ::std::sync::Arc<dyn ::arbordb::access::Writer + 't>,
            base:   ::arbordb::path::VPath,
            #phantom_field
        }

        impl #accessor_impl #mut_name #accessor_ty #accessor_where {
            #(#mut_getters)*
        }

        impl #accessor_impl ::arbordb::data::AMut<'t> for #mut_name #accessor_ty #accessor_where {
            fn open(
                writer: ::std::sync::Arc<dyn ::arbordb::access::Writer + 't>,
                base: ::arbordb::path::VPath,
            ) -> Self {
                Self {
                    writer,
                    base,
                    #phantom_init
                }
            }
        }

        impl #accessor_impl ::arbordb::data::AIdentifiable for #mut_name #accessor_ty #accessor_where {
            fn path(&self) -> &::arbordb::path::VPath {
                &self.base
            }
        }
    }
}
