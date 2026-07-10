//! Container-level `#[arbor(...)]` attributes (on the struct or enum itself).

use crate::attr::rename::RenameRule;
use crate::generics::Bounds;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, LitStr, Type};

/// Parsed container attributes.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    /// The casing rule applied to every field name / variant tag lacking an
    /// explicit `rename`. Defaults to [`RenameRule::None`] (verbatim).
    pub(crate) rename_all: RenameRule,
    /// `from = "U"`: load the value by reading a `U` and converting it infallibly.
    from:                  Option<Type>,
    /// `into = "U"`: store the value as a `U`.
    into:                  Option<Type>,
    /// `try_from = "U"`: load the value by reading a `U` and converting it fallibly.
    try_from:              Option<Type>,
    /// `tag = "..."`: the field name carrying the variant tag (internal / adjacent).
    tag:                   Option<String>,
    /// `content = "..."`: the field name carrying the payload (adjacent tagging).
    content:               Option<String>,
    /// `untagged`: the payload is stored bare, with no variant tag.
    untagged:              bool,
    /// `expecting = "..."`: overrides the "no variant matched" load error message.
    expecting:             Option<String>,
    /// `bound = "..."`: custom `where` predicates replacing the default `T: AData`
    /// on a generic type.
    bound:                 Option<Bounds>,
}

impl ContainerAttrs {
    /// Parses the `#[arbor(...)]` attributes attached to a container.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = Self::default();

        for attr in attrs {
            if !attr.path().is_ident("arbor") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    out.rename_all = RenameRule::from_lit(&meta.value()?.parse::<LitStr>()?)?;

                    return Ok(());
                }

                if meta.path.is_ident("from") {
                    out.from = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("into") {
                    out.into = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("try_from") {
                    out.try_from = Some(meta.value()?.parse::<LitStr>()?.parse()?);

                    return Ok(());
                }

                if meta.path.is_ident("tag") {
                    out.tag = Some(meta.value()?.parse::<LitStr>()?.value());

                    return Ok(());
                }

                if meta.path.is_ident("content") {
                    out.content = Some(meta.value()?.parse::<LitStr>()?.value());

                    return Ok(());
                }

                if meta.path.is_ident("untagged") {
                    out.untagged = true;

                    return Ok(());
                }

                if meta.path.is_ident("expecting") {
                    out.expecting = Some(meta.value()?.parse::<LitStr>()?.value());

                    return Ok(());
                }

                if meta.path.is_ident("bound") {
                    out.bound = Some(meta.value()?.parse::<LitStr>()?.parse_with(Bounds::parse_terminated)?);

                    return Ok(());
                }

                Err(meta.error("unknown arbor container attribute"))
            })?;
        }

        Ok(out)
    }

    /// The `into` target: the type the value is stored as.
    pub(crate) fn store_as(&self) -> Option<&Type> {
        self.into.as_ref()
    }

    /// The `from` target: the type `load` reconstructs from infallibly.
    pub(crate) fn load_from(&self) -> Option<&Type> {
        self.from.as_ref()
    }

    /// The `try_from` target: the type `load` reconstructs from fallibly.
    pub(crate) fn try_load_from(&self) -> Option<&Type> {
        self.try_from.as_ref()
    }

    /// Whether any of `from`/`into`/`try_from` makes this a delegated (stored-as-`U`) type.
    pub(crate) fn delegates(&self) -> bool {
        self.from.is_some() || self.into.is_some() || self.try_from.is_some()
    }

    /// The `tag` field name (internal / adjacent tagging).
    pub(crate) fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// The `content` field name (adjacent tagging).
    pub(crate) fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }

    /// Whether the enum is untagged.
    pub(crate) fn untagged(&self) -> bool {
        self.untagged
    }

    /// The error value for "no variant matched": the container's `expecting`
    /// message if set, otherwise `default`.
    pub(crate) fn no_match_error(&self, default: TokenStream) -> TokenStream {
        match &self.expecting {
            Some(message) => quote! { ::arbordb::AdbError::Corrupt(::std::string::String::from(#message)) },
            None => quote! { ::arbordb::AdbError::Corrupt(#default) },
        }
    }

    /// Custom `bound` predicates that replace the default `T: AData` on a generic type.
    pub(crate) fn bound(&self) -> Option<&Bounds> {
        self.bound.as_ref()
    }
}
