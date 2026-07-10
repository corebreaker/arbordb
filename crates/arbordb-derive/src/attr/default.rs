//! The `default` field attribute — how an absent or skipped field is produced on load.

use syn::Path;

/// How a field's value is produced on load when its vnode is absent or the field
/// is skipped.
pub(crate) enum FieldDefault {
    /// `#[arbor(default)]` — `::core::default::Default::default()`.
    Trait,
    /// `#[arbor(default = "path")]` — `path()`.
    Path(Path),
}
