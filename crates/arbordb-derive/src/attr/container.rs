//! Container-level `#[arbor(...)]` attributes (on the struct or enum itself).

use crate::attr::rename::RenameRule;

use syn::{Attribute, LitStr};

/// Parsed container attributes.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    /// The casing rule applied to every field name / variant tag lacking an
    /// explicit `rename`. Defaults to [`RenameRule::None`] (verbatim).
    pub(crate) rename_all: RenameRule,
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
                    let lit: LitStr = meta.value()?.parse()?;
                    out.rename_all = RenameRule::from_lit(&lit)?;

                    return Ok(());
                }

                Err(meta.error("unknown arbor container attribute"))
            })?;
        }

        Ok(out)
    }
}
