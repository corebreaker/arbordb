//! Field-level `#[arbor(...)]` attributes.

use crate::attr::default::FieldDefault;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, LitStr, Path, Token};

/// Parsed field attributes.
#[derive(Default)]
pub(crate) struct FieldAttrs {
    /// An explicit stored name, overriding the container's `rename_all`.
    pub(crate) rename:        Option<String>,
    /// Extra names accepted on load, in declaration order. The stored name is
    /// always the primary one; aliases are load-only.
    pub(crate) aliases:       Vec<String>,
    /// Never store or load this field; load produces the default.
    pub(crate) skip:          bool,
    /// Never store this field; load produces the default (like `skip`).
    pub(crate) skip_store:    bool,
    /// Store this field, but never read it on load; load produces the default.
    pub(crate) skip_load:     bool,
    /// Skip storing when this predicate (`fn(&T) -> bool`) returns true.
    pub(crate) skip_store_if: Option<Path>,
    /// How to produce the value on load when the node is absent or the field is
    /// skipped.
    pub(crate) default:       Option<FieldDefault>,
}

impl FieldAttrs {
    /// Parses the `#[arbor(...)]` attributes attached to a field.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = Self::default();

        for attr in attrs {
            if !attr.path().is_ident("arbor") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    out.rename = Some(meta.value()?.parse::<LitStr>()?.value());

                    return Ok(());
                }

                if meta.path.is_ident("alias") {
                    out.aliases.push(meta.value()?.parse::<LitStr>()?.value());

                    return Ok(());
                }

                if meta.path.is_ident("skip") {
                    out.skip = true;

                    return Ok(());
                }

                if meta.path.is_ident("skip_store") {
                    out.skip_store = true;

                    return Ok(());
                }

                if meta.path.is_ident("skip_load") {
                    out.skip_load = true;

                    return Ok(());
                }

                if meta.path.is_ident("skip_store_if") {
                    out.skip_store_if = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("default") {
                    out.default = Some(if meta.input.peek(Token![=]) {
                        FieldDefault::Path(meta.value()?.parse::<LitStr>()?.parse()?)
                    } else {
                        FieldDefault::Trait
                    });

                    return Ok(());
                }

                Err(meta.error("unknown arbor field attribute"))
            })?;
        }

        Ok(out)
    }

    /// Whether the field has a stored node — written on store, navigable by an
    /// accessor, listed in `Desc`. False for `skip` / `skip_store`.
    pub(crate) fn in_shape(&self) -> bool {
        !self.skip && !self.skip_store
    }

    /// Whether the field's value is read from its node on load. False for
    /// `skip` / `skip_store` / `skip_load` — those produce the default instead.
    pub(crate) fn loads_from_node(&self) -> bool {
        self.in_shape() && !self.skip_load
    }

    /// The default-value expression: `path()` for `default = "path"`, else
    /// `Default::default()` (for `default`, `skip`, `skip_store`, or `skip_load`).
    pub(crate) fn default_expr(&self) -> TokenStream {
        match &self.default {
            Some(FieldDefault::Path(path)) => quote! { #path() },
            _ => quote! { ::core::default::Default::default() },
        }
    }
}
