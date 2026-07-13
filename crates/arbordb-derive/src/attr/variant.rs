//! Variant-level `#[arbor(...)]` attributes (on an enum variant).

use syn::{Attribute, LitStr, Result as SynResult};

/// Parsed variant attributes.
#[derive(Default)]
pub(crate) struct VariantAttrs {
    /// An explicit stored tag, overriding the container's `rename_all`.
    rename:  Option<String>,
    /// Extra tags accepted on load, in declaration order. The stored tag is
    /// always the primary one; aliases are load-only.
    aliases: Vec<String>,
    /// Catch-all: an otherwise-unknown tag loads as this (unit) variant.
    other:   bool,
}

impl VariantAttrs {
    /// Parses the `#[arbor(...)]` attributes attached to a variant.
    pub(crate) fn parse(attrs: &[Attribute]) -> SynResult<Self> {
        let mut out = Self::default();

        for attr in attrs {
            if !attr.path().is_ident("arbor") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.rename = Some(lit.value());

                    return Ok(());
                }

                if meta.path.is_ident("alias") {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.aliases.push(lit.value());

                    return Ok(());
                }

                if meta.path.is_ident("other") {
                    out.other = true;

                    return Ok(());
                }

                // no-coverage:start — attribute-validation error (invalid input never compiles)
                Err(meta.error("unknown arbor variant attribute"))
                // no-coverage:stop
            })?;
        }

        Ok(out)
    }

    /// The explicit stored tag (`rename = "..."`), if set.
    pub(crate) fn rename(&self) -> Option<&str> {
        self.rename.as_deref()
    }

    /// Extra tags accepted on load, in declaration order.
    pub(crate) fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// Whether this is the `#[arbor(other)]` catch-all variant.
    pub(crate) fn other(&self) -> bool {
        self.other
    }
}
