//! Codegen for a delegated `AData` impl: `#[arbor(from / into / try_from = "U")]`.
//!
//! The value is stored entirely AS a target type `U` (`into`) and rebuilt from a
//! `U` on load (`from`, infallible, or `try_from`, fallible). This bypasses the
//! usual per-field / per-variant shredding — the field shape is never inspected,
//! so newtype/tuple structs and enums are all accepted. No `ArborXxx` /
//! `ArborXxxDesc` is generated; the accessors ARE `U`'s.

use crate::attr::ContainerAttrs;
use crate::generics::Generics;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error};

/// Expands a delegated `AData` impl for a container carrying `from`/`into`/`try_from`.
pub(crate) fn convert_impl(
    input: &DeriveInput,
    container: &ContainerAttrs,
    generics: &Generics,
) -> syn::Result<TokenStream> {
    let name = &input.ident;

    // The on-disk form is the `into` target — required so the value can be stored.
    let Some(into_ty) = container.store_as() else {
        let probe = container.load_from().or(container.try_load_from()).expect("delegates");

        return Err(Error::new_spanned(
            probe,
            "`from`/`try_from` needs a matching `into` to store the value",
        ));
    };

    // The load source is exactly one of `from` / `try_from`.
    let load_body = match (container.load_from(), container.try_load_from()) {
        (Some(_), Some(try_ty)) => {
            return Err(Error::new_spanned(
                try_ty,
                "`from` and `try_from` are mutually exclusive",
            ));
        }

        (Some(from_ty), None) => quote! {
            let value: #from_ty = <#from_ty as ::arbordb::data::AData>::load(reader, at)?;

            ::core::result::Result::Ok(::core::convert::From::from(value))
        },

        (None, Some(try_ty)) => quote! {
            let value: #try_ty = <#try_ty as ::arbordb::data::AData>::load(reader, at)?;

            <Self as ::core::convert::TryFrom<#try_ty>>::try_from(value)
                .map_err(|e| ::arbordb::AdbError::Conversion(::std::string::ToString::to_string(&e)))
        },

        (None, None) => {
            return Err(Error::new_spanned(
                into_ty,
                "`into` needs a matching `from` or `try_from` to load the value",
            ));
        }
    };

    let impl_generics = generics.adata_impl();
    let ty_generics = generics.adata_ty();
    let where_clause = generics.adata_where();

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::arbordb::data::AData for #name #ty_generics #where_clause {
            type Ref<'t> = <#into_ty as ::arbordb::data::AData>::Ref<'t>;
            type Mut<'t> = <#into_ty as ::arbordb::data::AData>::Mut<'t>;

            fn store<__W: ::arbordb::access::Writer>(
                &self,
                writer: &__W,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<()> {
                let target: #into_ty = ::core::convert::Into::into(::core::clone::Clone::clone(self));

                ::arbordb::data::AData::store(&target, writer, at)
            }

            fn load<__R: ::arbordb::access::Reader>(
                reader: &__R,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<Self> {
                #load_body
            }
        }
    })
}
