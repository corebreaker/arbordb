use super::super::WriteTxn;
use crate::{
    data::{AData, AMut},
    error::AdbResult,
    path::APath,
    Value,
};

/// A [`WriteTxn`] whose access paths are relative to a fixed root.
pub struct RootedWrite<'a> {
    txn:  &'a WriteTxn,
    root: APath,
}

impl<'a> RootedWrite<'a> {
    pub(crate) fn new(txn: &'a WriteTxn, root: APath) -> Self {
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

    /// Stores a typed value as a file at `path` (relative to the root).
    pub fn store<T: AData>(&self, path: impl AsRef<str>, value: &T) -> AdbResult<()> {
        self.txn.store(self.absolute(path)?, value)
    }

    /// Stores a dynamic [`Value`] at `path` (relative to the root).
    pub fn store_value(&self, path: impl AsRef<str>, value: &Value) -> AdbResult<()> {
        self.txn.store_value(self.absolute(path)?, value)
    }

    /// Creates the directory at `path` (relative to the root) and any ancestors.
    pub fn mkdir(&self, path: impl AsRef<str>) -> AdbResult<()> {
        self.txn.mkdir(self.absolute(path)?)
    }

    /// Removes the node at `path` (relative to the root). Reports if it existed.
    pub fn rm(&self, path: impl AsRef<str>) -> AdbResult<bool> {
        self.txn.rm(self.absolute(path)?)
    }

    /// Moves `src` to `dst` (both relative to the root), keeping its identity.
    pub fn mv(&self, src: impl AsRef<str>, dst: impl AsRef<str>) -> AdbResult<()> {
        self.txn.mv(self.absolute(src)?, self.absolute(dst)?)
    }

    /// Deep-copies the subtree at `src` to `dst` (both relative to the root).
    pub fn cp(&self, src: impl AsRef<str>, dst: impl AsRef<str>) -> AdbResult<()> {
        self.txn.cp(self.absolute(src)?, self.absolute(dst)?)
    }

    /// Opens a write accessor over the file at `path` (relative to the root).
    pub fn fetch_mut<A: AMut<'a>>(&self, path: impl AsRef<str>) -> AdbResult<Option<A>> {
        self.txn.fetch_mut(self.absolute(path)?)
    }

    fn absolute(&self, path: impl AsRef<str>) -> AdbResult<String> {
        Ok(self.root.join(&APath::parse(path.as_ref())?).to_string())
    }
}
