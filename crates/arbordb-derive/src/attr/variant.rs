//! Variant-level `#[arbor(...)]` attributes (on an enum variant).

use syn::{Attribute, LitStr};

/// Parsed variant attributes.
#[derive(Default)]
pub(crate) struct VariantAttrs {
    /// An explicit stored tag, overriding the container's `rename_all`.
    pub(crate) rename:  Option<String>,
    /// Extra tags accepted on load, in declaration order. The stored tag is
    /// always the primary one; aliases are load-only.
    pub(crate) aliases: Vec<String>,
}

impl VariantAttrs {
    /// Parses the `#[arbor(...)]` attributes attached to a variant.
    pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
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

                Err(meta.error("unknown arbor variant attribute"))
            })?;
        }

        Ok(out)
    }
}
