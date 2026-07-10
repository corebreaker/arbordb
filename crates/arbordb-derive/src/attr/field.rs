//! Field-level `#[arbor(...)]` attributes.

use syn::{Attribute, LitStr};

/// Parsed field attributes.
#[derive(Default)]
pub(crate) struct FieldAttrs {
    /// An explicit stored name, overriding the container's `rename_all`.
    pub(crate) rename:  Option<String>,
    /// Extra names accepted on load, in declaration order. The stored name is
    /// always the primary one; aliases are load-only.
    pub(crate) aliases: Vec<String>,
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
                    let lit: LitStr = meta.value()?.parse()?;
                    out.rename = Some(lit.value());

                    return Ok(());
                }

                if meta.path.is_ident("alias") {
                    let lit: LitStr = meta.value()?.parse()?;
                    out.aliases.push(lit.value());

                    return Ok(());
                }

                Err(meta.error("unknown arbor field attribute"))
            })?;
        }

        Ok(out)
    }
}
