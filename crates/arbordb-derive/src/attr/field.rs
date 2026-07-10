//! Field-level `#[arbor(...)]` attributes.

use crate::attr::default::FieldDefault;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{parse_quote, Attribute, LitStr, Path, Token};

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
    /// Custom store function (`store_with = "path"`) replacing `AData::store`.
    pub(crate) store_with:    Option<Path>,
    /// Custom load function (`load_with = "path"`) replacing `AData::load`.
    pub(crate) load_with:     Option<Path>,
    /// Module supplying both `store` and `load` (`with = "module"`); sugar for the
    /// two above.
    pub(crate) with:          Option<Path>,
    /// Flatten the field's (object) value into the parent's node — stored and
    /// loaded at the parent's path rather than a named child.
    pub(crate) flatten:       Option<Span>,
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

                if meta.path.is_ident("store_with") {
                    out.store_with = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("load_with") {
                    out.load_with = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("with") {
                    out.with = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("flatten") {
                    out.flatten = Some(meta.path.span());

                    return Ok(());
                }

                Err(meta.error("unknown arbor field attribute"))
            })?;
        }

        out.check_conflicts()?;

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

    /// The custom store function: `store_with`, else `with`'s `::store`, else none.
    pub(crate) fn store_fn(&self) -> Option<Path> {
        if let Some(path) = &self.store_with {
            return Some(path.clone());
        }

        self.with.as_ref().map(|module| parse_quote! { #module::store })
    }

    /// The custom load function: `load_with`, else `with`'s `::load`, else none.
    pub(crate) fn load_fn(&self) -> Option<Path> {
        if let Some(path) = &self.load_with {
            return Some(path.clone());
        }

        self.with.as_ref().map(|module| parse_quote! { #module::load })
    }

    /// Whether the field is flattened into the parent's node (stored and loaded at
    /// the parent's path rather than a named child).
    pub(crate) fn is_flatten(&self) -> bool {
        self.flatten.is_some()
    }

    /// Rejects attribute combinations that cannot both hold.
    fn check_conflicts(&self) -> syn::Result<()> {
        // A flattened field has no named node of its own, so no other attribute applies.
        if let Some(span) = self.flatten
            && (self.rename.is_some()
                || !self.aliases.is_empty()
                || self.skip
                || self.skip_store
                || self.skip_load
                || self.skip_store_if.is_some()
                || self.default.is_some()
                || self.with.is_some()
                || self.store_with.is_some()
                || self.load_with.is_some())
        {
            return Err(syn::Error::new(
                span,
                "`flatten` cannot be combined with other field attributes",
            ));
        }

        // `with` already sets both sides; an explicit `store_with`/`load_with` is redundant.
        if self.with.is_some()
            && let Some(dup) = self.store_with.as_ref().or(self.load_with.as_ref())
        {
            return Err(syn::Error::new_spanned(
                dup,
                "`store_with`/`load_with` cannot be combined with `with`",
            ));
        }

        // A custom store only runs for a stored field.
        if let Some(store) = self.store_with.as_ref().or(self.with.as_ref())
            && (self.skip || self.skip_store)
        {
            return Err(syn::Error::new_spanned(
                store,
                "`store_with`/`with` conflicts with `skip`/`skip_store`: the field is never stored",
            ));
        }

        // A custom load only runs for a loaded field.
        if let Some(load) = self.load_with.as_ref().or(self.with.as_ref())
            && (self.skip || self.skip_load)
        {
            return Err(syn::Error::new_spanned(
                load,
                "`load_with`/`with` conflicts with `skip`/`skip_load`: the field is never loaded",
            ));
        }

        Ok(())
    }
}
