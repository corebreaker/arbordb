//! [`IndexId`] — the compact numeric handle a named index is mapped to internally,
//! keeping physical index keys short.

/// Internal compact identifier for a named index.
///
/// Index names are mapped to a small numeric id in `$metadata` so that index keys
/// stay short.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct IndexId(pub(crate) u32);
