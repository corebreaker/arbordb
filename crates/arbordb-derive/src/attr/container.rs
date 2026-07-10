//! Container-level `#[arbor(...)]` attributes (on the struct or enum itself).

use crate::attr::rename::RenameRule;

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
}
