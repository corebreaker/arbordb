//! Top-level `#[derive(AData)]` expansion for enums: the `AData` impl (store
//! dispatch + load body per representation), the minimal `variant()`-only
//! accessors, and the descriptor.

use super::{
    variant::VariantInfo,
    load::{load_arm, untagged_arm},
};

use crate::{
    attr::{ContainerAttrs, VariantAttrs},
    enums::{repr::EnumRepr, store},
    generics::Generics,
    desc::desc_enum,
};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{DataEnum, DeriveInput, Error, Ident, Result as SynResult};

/// Expands `#[derive(AData)]` for an enum into its impl, accessors, and descriptor.
pub(crate) fn expand_enum(
    input: &DeriveInput,
    data: &DataEnum,
    container: &ContainerAttrs,
    generics: &Generics,
) -> SynResult<TokenStream> {
    let name = &input.ident;
    let vis = &input.vis;
    let ref_name = format_ident!("Arbor{}", name);
    let mut_name = format_ident!("Arbor{}Mut", name);
    let desc_name = format_ident!("Arbor{}Desc", name);

    let repr = EnumRepr::from_container(container, name)?;

    let variants = data
        .variants
        .iter()
        .map(|variant| {
            let attrs = VariantAttrs::parse(&variant.attrs)?;

            let tag = attrs
                .rename()
                .map(str::to_string)
                .unwrap_or_else(|| container.rename_all.apply_to_variant(&variant.ident.to_string()));

            Ok(VariantInfo::new(variant, tag, attrs.aliases().to_vec(), attrs.other()))
        })
        .collect::<SynResult<Vec<_>>>()?;

    let other_variant = other_variant(&variants, &repr)?;

    let store_arms = variants.iter().map(|info| store::store_arm(info, &repr));
    let store_prelude = repr.store_prelude();
    let load_body = load_body(&variants, &repr, container, other_variant);
    let variant_names: Vec<String> = variants.iter().map(|info| info.tag().to_string()).collect();

    let impl_generics = generics.adata_impl();
    let ty_generics = generics.adata_ty();
    let where_clause = generics.adata_where();
    let accessor_ty = generics.accessor_ty();

    let adata = quote! {
        #[automatically_derived]
        impl #impl_generics ::arbordb::data::AData for #name #ty_generics #where_clause {
            type Ref<'t> = #ref_name #accessor_ty;
            type Mut<'t> = #mut_name #accessor_ty;

            fn store<__W: ::arbordb::access::Writer>(
                &self,
                writer: &__W,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<()> {
                ::arbordb::access::Writer::remove(writer, at)?;
                #store_prelude

                match self {
                    #(#store_arms)*
                }

                ::core::result::Result::Ok(())
            }

            fn load<__R: ::arbordb::access::Reader>(
                reader: &__R,
                at: &::arbordb::path::VPath,
            ) -> ::arbordb::AdbResult<Self> {
                #load_body
            }
        }
    };

    let accessors = accessors(input, &ref_name, &mut_name, &repr, generics);
    let desc = desc_enum(vis, &desc_name, &name.to_string(), &variant_names);

    Ok(quote! {
        #adata
        #accessors
        #desc
    })
}

/// Validates the `#[arbor(other)]` catch-all — at most one, unit, never untagged —
/// and returns its identifier if present.
fn other_variant<'a>(variants: &'a [VariantInfo<'a>], repr: &EnumRepr) -> SynResult<Option<&'a Ident>> {
    let mut others = variants.iter().filter(|info| info.other());

    match (others.next(), others.next()) {
        (None, _) => Ok(None),
        (Some(_), Some(second)) => Err(Error::new(
            second.ident().span(),
            "at most one variant may be `#[arbor(other)]`",
        )),
        (Some(first), None) => {
            if repr.is_untagged() {
                return Err(Error::new(
                    first.ident().span(),
                    "`#[arbor(other)]` is not supported on untagged enums",
                ));
            }

            if !first.is_unit() {
                return Err(Error::new(
                    first.ident().span(),
                    "an `#[arbor(other)]` variant must be a unit variant",
                ));
            }

            Ok(Some(first.ident()))
        }
    }
}

/// The body of the `load` fn: try-each-in-order for untagged, else a tag match
/// with a catch-all (the `other` variant or a "no match" error).
fn load_body(
    variants: &[VariantInfo],
    repr: &EnumRepr,
    container: &ContainerAttrs,
    other_variant: Option<&Ident>,
) -> TokenStream {
    if repr.is_untagged() {
        let attempts = variants.iter().map(untagged_arm);
        let error = container.no_match_error(quote! { ::std::format!("no untagged variant matched at '{at}'") });

        return quote! {
            #(#attempts)*

            ::core::result::Result::Err(#error)
        };
    }

    let tag_load = repr.tag_load();

    // The `other` variant is the match's catch-all, so it gets no arm of its own.
    let load_arms = variants
        .iter()
        .filter(|info| !info.other())
        .map(|info| load_arm(info, repr));

    let catch_all = match other_variant {
        Some(id) => quote! {
            _ => ::core::result::Result::Ok(Self::#id),
        },
        None => {
            let error = container.no_match_error(quote! { ::std::format!("unknown enum variant tag: {tag}") });

            quote! {
                _ => ::core::result::Result::Err(#error),
            }
        }
    };

    quote! {
        #tag_load

        match tag.as_str() {
            #(#load_arms)*
            #catch_all
        }
    }
}

/// The minimal enum accessors: read and write cursors exposing only `variant()`
/// (the active tag). The full payload is recomposed with `load::<E>`.
fn accessors(
    input: &DeriveInput,
    ref_name: &Ident,
    mut_name: &Ident,
    repr: &EnumRepr,
    generics: &Generics,
) -> TokenStream {
    let vis = &input.vis;
    let ref_variant = repr.variant_body(quote! { self.reader });
    let mut_variant = repr.variant_body(quote! { self.writer });

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
            /// The active variant's tag name.
            #vis fn variant(&self) -> ::arbordb::AdbResult<::std::string::String> {
                #ref_variant
            }
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
            /// The active variant's tag name.
            #vis fn variant(&self) -> ::arbordb::AdbResult<::std::string::String> {
                #mut_variant
            }
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
