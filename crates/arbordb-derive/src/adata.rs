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

        // Fast path: no aliases — load straight from the stored name.
        if field.aliases.is_empty() {
            return quote! {
                #ident: <#ty as ::arbordb::data::AData>::load(reader, &at.child_name(#stored))?,
            };
        }

        // Alias path: pick the primary name, else the first alias that exists,
        // else fall back to the primary name (yielding the normal not-found path).
        let aliases = &field.aliases;

        quote! {
            #ident: {
                let mut __chosen: ::core::option::Option<&str> = ::core::option::Option::None;
                for __candidate in [#stored, #(#aliases),*] {
                    if ::arbordb::access::Reader::exists_at(reader, &at.child_name(__candidate))? {
                        __chosen = ::core::option::Option::Some(__candidate);

                        break;
                    }
                }

                let __at = match __chosen {
                    ::core::option::Option::Some(__candidate) => at.child_name(__candidate),
                    ::core::option::Option::None => at.child_name(#stored),
                };

                <#ty as ::arbordb::data::AData>::load(reader, &__at)?
            },
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
