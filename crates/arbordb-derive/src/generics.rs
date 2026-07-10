//! Propagation of a type's generics and bounds onto the generated `AData` impl
//! and accessor types.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{GenericParam, Token, WherePredicate, punctuated::Punctuated};

/// A parsed `bound = "..."` predicate list (e.g. `T: AData, U: Clone`).
pub(crate) type Bounds = Punctuated<WherePredicate, Token![,]>;

/// Pre-rendered generics fragments for the code generators. Built once per type
/// by [`Generics::analyze`]; for a non-generic type every fragment is empty (bar
/// the accessors' `'t`), so the generated code is unchanged.
pub(crate) struct Generics {
    /// The impl generics for the `AData` impl on the original type (`<T>`).
    a_data_impl:    TokenStream,
    /// The type generics for the original type (`<T>`).
    a_data_ty:      TokenStream,
    /// The where-clause for the `AData` impl (`where T: AData`, or the custom bound).
    a_data_where:   TokenStream,
    /// The accessor impl generics, including the extra `'t` (`<'t, T>`).
    accessor_impl:  TokenStream,
    /// The accessor type generics (`<'t, T>`).
    accessor_ty:    TokenStream,
    /// The accessor where-clause (identical to the `AData` one).
    accessor_where: TokenStream,
    /// A trailing `__marker: PhantomData<…>,` field (empty without type params).
    phantom_field:  TokenStream,
    /// The matching `__marker: PhantomData,` initializer (empty without type params).
    phantom_init:   TokenStream,
}

impl Generics {
    /// Builds the fragments from a type's declared generics, applying either the
    /// custom `bound` predicates (which replace the synthesized bounds) or the
    /// default `T: AData` on every type parameter.
    pub(crate) fn analyze(generics: &syn::Generics, bound: Option<&Bounds>) -> Self {
        let mut adata = generics.clone();

        {
            let where_clause = adata.make_where_clause();

            match bound {
                Some(predicates) => where_clause.predicates.extend(predicates.iter().cloned()),
                None => {
                    for param in &generics.params {
                        if let GenericParam::Type(type_param) = param {
                            let ident = &type_param.ident;

                            where_clause
                                .predicates
                                .push(syn::parse_quote! { #ident: ::arbordb::data::AData });
                        }
                    }
                }
            }
        }

        // Accessors carry an extra `'t` for the borrowed reader/writer.
        let mut accessor = adata.clone();
        accessor.params.insert(0, syn::parse_quote!('t));

        let (adata_impl, adata_ty, adata_where) = adata.split_for_impl();
        let (accessor_impl, accessor_ty, accessor_where) = accessor.split_for_impl();

        // Type parameters appear only in the accessors' getter return types, never
        // their fields, so a `PhantomData` keeps each one "used" (avoids E0392).
        let type_params: Vec<&syn::Ident> = generics.type_params().map(|param| &param.ident).collect();
        let (phantom_field, phantom_init) = if type_params.is_empty() {
            (quote! {}, quote! {})
        } else {
            (
                quote! { __marker: ::core::marker::PhantomData<fn() -> ( #(#type_params,)* )>, },
                quote! { __marker: ::core::marker::PhantomData, },
            )
        };

        Self {
            a_data_impl: quote! { #adata_impl },
            a_data_ty: quote! { #adata_ty },
            a_data_where: quote! { #adata_where },
            accessor_impl: quote! { #accessor_impl },
            accessor_ty: quote! { #accessor_ty },
            accessor_where: quote! { #accessor_where },
            phantom_field,
            phantom_init,
        }
    }

    /// The impl generics for the `AData` impl on the original type (`<T>`).
    pub(crate) fn adata_impl(&self) -> &TokenStream {
        &self.a_data_impl
    }

    /// The type generics for the original type (`<T>`).
    pub(crate) fn adata_ty(&self) -> &TokenStream {
        &self.a_data_ty
    }

    /// The where-clause for the `AData` impl (`where T: AData`, or the custom bound).
    pub(crate) fn adata_where(&self) -> &TokenStream {
        &self.a_data_where
    }

    /// The accessor impl generics, including the extra `'t` (`<'t, T>`).
    pub(crate) fn accessor_impl(&self) -> &TokenStream {
        &self.accessor_impl
    }

    /// The accessor type generics (`<'t, T>`), used at the GAT reference and impl target.
    pub(crate) fn accessor_ty(&self) -> &TokenStream {
        &self.accessor_ty
    }

    /// The accessor where-clause (identical to the `AData` one).
    pub(crate) fn accessor_where(&self) -> &TokenStream {
        &self.accessor_where
    }

    /// A trailing `__marker: PhantomData<…>,` field (empty without type params).
    pub(crate) fn phantom_field(&self) -> &TokenStream {
        &self.phantom_field
    }

    /// The matching `__marker: PhantomData,` initializer (empty without type params).
    pub(crate) fn phantom_init(&self) -> &TokenStream {
        &self.phantom_init
    }
}
