use super::super::{IndexQuery, ReadTxn};
use crate::{
    data::{AData, AValue, ARef, Scalar},
    error::AdbResult,
    path::APath,
    EntryKind,
    Value,
};

/// A [`ReadTxn`](crate::txn::ReadTxn) whose access paths are relative to a fixed root.
pub struct RootedRead<'a> {
    txn:  &'a ReadTxn,
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
    pub fn rooted(&self, sub: impl AsRef<str>) -> AdbResult<Self> {
        Ok(Self {
            txn:  self.txn,
            root: self.root.join(&APath::parse(sub.as_ref())?),
        })
    }

    /// Loads a typed value from the file at `path` (relative to the root).
    pub fn load<T: AData>(&self, path: impl AsRef<str>) -> AdbResult<Option<T>> {
        self.txn.load(self.absolute(path)?)
    }

    /// Loads the dynamic [`Value`] at `path` (relative to the root).
    pub fn load_value(&self, path: impl AsRef<str>) -> AdbResult<Option<Value>> {
        self.txn.load_value(self.absolute(path)?)
    }

    /// Opens a read accessor over the file at `path` (relative to the root).
    pub fn fetch<A: ARef<'static>>(&self, path: impl AsRef<str>) -> AdbResult<Option<A>> {
        self.txn.fetch(self.absolute(path)?)
    }

    /// Reads the scalar at `at` inside the file at `path` (relative to the root).
    pub fn get(&self, path: impl AsRef<str>, at: impl AsRef<str>) -> AdbResult<Option<Scalar>> {
        self.txn.get(self.absolute(path)?, at)
    }

    /// Reads a typed scalar at `at` inside the file at `path` (relative to the root).
    pub fn get_as<V: AValue>(&self, path: impl AsRef<str>, at: impl AsRef<str>) -> AdbResult<Option<V>> {
        self.txn.get_as(self.absolute(path)?, at)
    }

    /// The kind of node at `path` (relative to the root), if any.
    pub fn kind(&self, path: impl AsRef<str>) -> AdbResult<Option<EntryKind>> {
        self.txn.kind(self.absolute(path)?)
    }

    /// Whether a node exists at `path` (relative to the root).
    pub fn exists(&self, path: impl AsRef<str>) -> AdbResult<bool> {
        self.txn.exists(self.absolute(path)?)
    }

    /// Lists the direct children of the directory at `path` (relative to the root).
    pub fn ls(&self, path: impl AsRef<str>) -> AdbResult<Vec<(String, EntryKind)>> {
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

    fn absolute(&self, path: impl AsRef<str>) -> AdbResult<String> {
        Ok(self.root.join(&APath::parse(path.as_ref())?).to_string())
    }
}
