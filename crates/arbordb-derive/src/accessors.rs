//! Codegen for the read (`ArborXxx`) and write (`ArborXxxMut`) accessor types.
//!
//! An accessor pairs a shared cursor with a base `VPath`. A getter is
//! **infallible** — it just clones the cursor and extends the path (no I/O); the
//! read happens later on the returned accessor (`.get()` on a leaf, a nested
//! getter, …). This is the ArborDb difference from StratoDb: navigation is by
//! `VPath`, so no per-node key is resolved or stored.

use crate::fields::Field;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Visibility};

/// Generates both accessor types, their getters, and their trait impls.
pub(crate) fn accessors(vis: &Visibility, ref_name: &Ident, mut_name: &Ident, fields: &[Field]) -> TokenStream {
    let ref_getters = fields.iter().map(|field| {
        let getter = field.ident;
        let ty = field.ty;
        let stored = &field.name;

        quote! {
            #vis fn #getter(&self) -> <#ty as ::arbordb::data::AData>::Ref<'t> {
                <<#ty as ::arbordb::data::AData>::Ref<'t> as ::arbordb::data::ARef<'t>>::open(
                    ::std::sync::Arc::clone(&self.reader),
                    self.base.child_name(#stored),
                )
            }
        }
    });

    let mut_getters = fields.iter().map(|field| {
        let getter = format_ident!("{}_mut", field.ident);
        let ty = field.ty;
        let stored = &field.name;

        quote! {
            #vis fn #getter(&self) -> <#ty as ::arbordb::data::AData>::Mut<'t> {
                <<#ty as ::arbordb::data::AData>::Mut<'t> as ::arbordb::data::AMut<'t>>::open(
                    ::std::sync::Arc::clone(&self.writer),
                    self.base.child_name(#stored),
                )
            }
        }
    });

    quote! {
        #[allow(dead_code)]
        #vis struct #ref_name<'t> {
            reader: ::std::sync::Arc<dyn ::arbordb::access::Reader + 't>,
            base:   ::arbordb::path::VPath,
        }

        impl<'t> #ref_name<'t> {
            #(#ref_getters)*
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
            #(#mut_getters)*
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
