//! A resolved enum variant: its AST node paired with the `#[arbor(...)]` naming
//! (stored tag, load-only aliases, and the `other` catch-all flag).

use syn::{Fields, Ident, Variant};

/// One variant with its `#[arbor(...)]` naming resolved.
pub(super) struct VariantInfo<'a> {
    /// The variant AST node (identifier + fields).
    variant: &'a Variant,
    /// The stored tag (primary name written on store).
    tag:     String,
    /// Extra tags accepted on load, in declaration order.
    aliases: Vec<String>,
    /// Whether this is the `#[arbor(other)]` catch-all variant.
    other:   bool,
}

impl<'a> VariantInfo<'a> {
    /// Pairs a variant's AST node with its resolved `#[arbor(...)]` naming.
    pub(super) fn new(variant: &'a Variant, tag: String, aliases: Vec<String>, other: bool) -> Self {
        Self {
            variant,
            tag,
            aliases,
            other,
        }
    }

    /// The stored tag (primary name written on store).
    pub(super) fn tag(&self) -> &str {
        &self.tag
    }

    /// Whether the variant carries no payload.
    pub(super) fn is_unit(&self) -> bool {
        matches!(self.variant.fields, Fields::Unit)
    }

    /// The variant's fields (unit, tuple, or named).
    pub(super) fn fields(&self) -> &Fields {
        &self.variant.fields
    }

    /// Extra tags accepted on load, in declaration order.
    pub(super) fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// The variant identifier.
    pub(super) fn ident(&self) -> &Ident {
        &self.variant.ident
    }

    /// Whether this is the `#[arbor(other)]` catch-all variant.
    pub(super) fn other(&self) -> bool {
        self.other
    }
}
