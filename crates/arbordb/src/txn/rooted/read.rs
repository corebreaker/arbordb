//! [`RootedRead`] — a read view that prefixes every access path with a fixed root
//! before forwarding to the underlying [`ReadTxn`], including index queries scoped
//! to the root's subtree.

use super::super::{IndexQuery, ReadTxn};
use crate::{
    data::{AData, AValue, ARef, Scalar},
    error::AdbResult,
    entry::{Entry, EntryKind},
    export::{JsonExporter, YamlExporter},
    path::{APath, IntoArborPath, IntoValuePath},
    Value,
};

/// A [`ReadTxn`] whose access paths are relative to a fixed root.
pub struct RootedRead<'a> {
    /// The transaction every path is forwarded to.
    txn:  &'a ReadTxn,
    /// The fixed root prepended to each access path.
    root: APath,
}

impl<'a> RootedRead<'a> {
    pub(crate) fn new(txn: &'a ReadTxn, root: APath) -> Self {
        Self {
            txn,
            root,
        }
    }

    /// The root this view is anchored at.
    pub fn root(&self) -> &APath {
        &self.root
    }

    /// A further view, rooted at `sub` relative to this view's root.
    pub fn rooted(&self, sub: impl IntoArborPath) -> AdbResult<Self> {
        Ok(Self {
            txn:  self.txn,
            root: self.root.join(&sub.into_arbor_path()?),
        })
    }

    /// Loads a typed value from the file at `path` (relative to the root).
    pub fn load<T: AData>(&self, path: impl IntoArborPath) -> AdbResult<Option<T>> {
        self.txn.load(self.absolute(path)?)
    }

    /// Loads the dynamic [`Value`] at `path` (relative to the root).
    pub fn load_value(&self, path: impl IntoArborPath) -> AdbResult<Option<Value>> {
        self.txn.load_value(self.absolute(path)?)
    }

    /// Loads a [`serde::de::DeserializeOwned`] value at `path` (relative to the root).
    #[cfg(feature = "serde")]
    pub fn load_serde_value<T: serde::de::DeserializeOwned>(&self, path: impl IntoArborPath) -> AdbResult<Option<T>> {
        self.txn.load_serde_value(self.absolute(path)?)
    }

    /// Opens a read accessor over the file at `path` (relative to the root).
    pub fn fetch<A: ARef<'static>>(&self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        self.txn.fetch(self.absolute(path)?)
    }

    /// Reads the scalar at `at` inside the file at `path` (relative to the root).
    pub fn get(&self, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<Scalar>> {
        self.txn.get(self.absolute(path)?, at)
    }

    /// Reads a typed scalar at `at` inside the file at `path` (relative to the root).
    pub fn get_as<V: AValue>(&self, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<V>> {
        self.txn.get_as(self.absolute(path)?, at)
    }

    /// The kind of vnode at `path` (relative to the root), if any.
    pub fn kind(&self, path: impl IntoArborPath) -> AdbResult<Option<EntryKind>> {
        self.txn.kind(self.absolute(path)?)
    }

    /// Whether a vnode exists at `path` (relative to the root).
    pub fn exists(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        self.txn.exists(self.absolute(path)?)
    }

    /// Lists the direct children of the directory at `path` (relative to the root).
    pub fn ls(&self, path: impl IntoArborPath) -> AdbResult<Vec<Entry>> {
        self.txn.ls(self.absolute(path)?)
    }

    /// Starts an [`IndexQuery`] scoped to this view's root: only entities at or
    /// under the root are kept.
    pub fn query(&self, index: &str) -> IndexQuery<'a> {
        self.txn.query(index).under(self.root.clone())
    }

    /// Finds the entities an index points at, keeping only those at or under this
    /// view's root, each recomposed as a `T`.
    pub fn find<T: AData>(&self, index: &str, values: &[Scalar]) -> AdbResult<Vec<T>> {
        self.query(index).prefixed(values).run()
    }

    /// Resolves `path` against this view's root into an absolute access path.
    fn absolute(&self, path: impl IntoArborPath) -> AdbResult<APath> {
        Ok(self.root.join(&path.into_arbor_path()?))
    }
}

impl<P: IntoArborPath> JsonExporter<P> for RootedRead<'_> {
    /// Exports the value stored at `path` (relative to the root) as JSON.
    fn export_to_json(&self, path: P, indent: Option<usize>) -> AdbResult<String> {
        self.txn.export_to_json(self.absolute(path)?, indent)
    }
}

impl<P: IntoArborPath> YamlExporter<P> for RootedRead<'_> {
    /// Exports the value stored at `path` (relative to the root) as YAML.
    fn export_to_yaml(&self, path: P) -> AdbResult<String> {
        self.txn.export_to_yaml(self.absolute(path)?)
    }
}
