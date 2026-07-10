//! The [`JsonExporter`] / [`YamlExporter`] traits and their implementations.
//!
//! Both a [`ReadTxn`] (rendering the value stored at an access path) and a
//! [`Value`] (rendering an in-memory subtree) export through the same two writers
//! in `json`/`yaml`. They differ only in how the address is interpreted: a
//! `ReadTxn` takes an [`APath`] to a stored file, a `Value` takes a [`VPath`] into
//! itself — so each trait is generic over the path type its implementor accepts.

use super::{json::to_json, yaml::to_yaml};
use crate::{
    engine::EntryKind,
    error::{AdbError, AdbResult},
    path::{APath, IntoArborPath, IntoValuePath, VPath},
    txn::ReadTxn,
    value::Value,
};

/// Renders a document to JSON.
///
/// Implemented by [`ReadTxn`] (rendering the value stored at an [`APath`]) and by
/// [`Value`] (rendering the in-memory subtree at a [`VPath`]).
pub trait JsonExporter<P> {
    /// Exports the value addressed by `path` as a JSON document. `indent` selects
    /// the layout: `None` yields compact JSON (no whitespace), `Some(n)`
    /// pretty-prints it with `n` spaces of indentation per nesting level.
    ///
    /// Object fields come out in sorted order. Scalars without a native JSON form
    /// take a textual one: dates and times as ISO 8601 / RFC 3339, a UUID
    /// hyphenated, raw bytes as Base64, a duration as its number of seconds, the
    /// non-finite floats (`NaN`, `±∞`) as `null`.
    ///
    /// On a [`ReadTxn`], `path` must name a file: a directory is refused with
    /// [`CannotAccess`](crate::AdbError::CannotAccess) and an absent path with
    /// [`ValueNotFound`](crate::AdbError::ValueNotFound). On a [`Value`], a `path`
    /// that leads nowhere errors with
    /// [`PathNotFound`](crate::AdbError::PathNotFound).
    fn export_to_json(&self, path: P, indent: Option<usize>) -> AdbResult<String>;
}

/// Renders a document to YAML, in block style.
///
/// Implemented by [`ReadTxn`] and [`Value`], like [`JsonExporter`].
pub trait YamlExporter<P> {
    /// Exports the value addressed by `path` as a YAML document (block style).
    /// Object fields come out in sorted order and every string is double-quoted;
    /// scalar rendering and the addressing rules match
    /// [`JsonExporter::export_to_json`].
    fn export_to_yaml(&self, path: P) -> AdbResult<String>;
}

impl<P: IntoArborPath> JsonExporter<P> for ReadTxn {
    fn export_to_json(&self, path: P, indent: Option<usize>) -> AdbResult<String> {
        Ok(to_json(&txn_value(self, path.into_arbor_path()?)?, indent))
    }
}

impl<P: IntoArborPath> YamlExporter<P> for ReadTxn {
    fn export_to_yaml(&self, path: P) -> AdbResult<String> {
        Ok(to_yaml(&txn_value(self, path.into_arbor_path()?)?))
    }
}

impl<P: IntoValuePath> JsonExporter<P> for Value {
    fn export_to_json(&self, path: P, indent: Option<usize>) -> AdbResult<String> {
        Ok(to_json(navigate(self, &path.into_value_path()?)?, indent))
    }
}

impl<P: IntoValuePath> YamlExporter<P> for Value {
    fn export_to_yaml(&self, path: P) -> AdbResult<String> {
        Ok(to_yaml(navigate(self, &path.into_value_path()?)?))
    }
}

/// The [`Value`] a read transaction exports for `path`: the whole value stored in
/// the file there. A directory has no value of its own — its children are separate
/// vnodes — so it is refused; a path that resolves to nothing is not found.
fn txn_value(txn: &ReadTxn, path: APath) -> AdbResult<Value> {
    match txn.kind(&path)? {
        Some(EntryKind::File) => txn.load_value(&path)?.ok_or_else(|| AdbError::ValueNotFound(path)),
        Some(EntryKind::Dir) => Err(AdbError::CannotAccess(format!(
            "'{path}' is a directory and cannot be exported"
        ))),
        None => Err(AdbError::ValueNotFound(path)),
    }
}

/// Resolves `base` within an in-memory [`Value`] to the addressed subtree, or
/// [`PathNotFound`](crate::AdbError::PathNotFound) if a segment leads nowhere. The
/// root path returns the whole value.
fn navigate<'v>(value: &'v Value, base: &VPath) -> AdbResult<&'v Value> {
    value.subtree(base).ok_or_else(|| AdbError::PathNotFound(base.clone()))
}
