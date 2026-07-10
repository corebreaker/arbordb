//! Codegen for the struct's `AData` impl (store / load).

use crate::fields::Field;
use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

/// Generates `impl AData for Struct`, storing/loading one child node per field.
pub(crate) fn adata_impl(name: &Ident, ref_name: &Ident, mut_name: &Ident, fields: &[Field]) -> TokenStream {
    let stores = fields.iter().map(|field| {
        let ident = field.ident;
        let stored = &field.name;

        quote! {
            ::arbordb::data::AData::store(&self.#ident, writer, &at.child_name(#stored))?;
        }
    });

    let loads = fields.iter().map(|field| {
        let ident = field.ident;
        let ty = field.ty;
        let stored = &field.name;

        quote! {
            #ident: <#ty as ::arbordb::data::AData>::load(reader, &at.child_name(#stored))?,
        }
    });

    quote! {
        #[automatically_derived]
        impl ::arbordb::data::AData for #name {
            type Ref<'t> = #ref_name<'t>;
            type Mut<'t> = #mut_name<'t>;

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
        }
    }
}
