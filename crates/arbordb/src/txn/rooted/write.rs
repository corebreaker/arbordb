use super::super::WriteTxn;
use crate::{
    data::{AData, AMut},
    error::AdbResult,
    path::{APath, IntoArborPath},
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
    pub fn rooted(&self, sub: impl IntoArborPath) -> AdbResult<Self> {
        Ok(Self {
            txn:  self.txn,
            root: self.root.join(&sub.into_arbor_path()?),
        })
    }

    /// Stores a typed value as a file at `path` (relative to the root).
    pub fn store<T: AData>(&self, path: impl IntoArborPath, value: &T) -> AdbResult<()> {
        self.txn.store(self.absolute(path)?, value)
    }

    /// Stores a dynamic [`Value`] at `path` (relative to the root).
    pub fn store_value(&self, path: impl IntoArborPath, value: &Value) -> AdbResult<()> {
        self.txn.store_value(self.absolute(path)?, value)
    }

    /// Creates the directory at `path` (relative to the root) and any ancestors.
    pub fn mkdir(&self, path: impl IntoArborPath) -> AdbResult<()> {
        self.txn.mkdir(self.absolute(path)?)
    }

    /// Removes the vnode at `path` (relative to the root). Reports if it existed.
    pub fn rm(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        self.txn.rm(self.absolute(path)?)
    }

    /// Moves `src` to `dst` (both relative to the root), keeping its identity.
    pub fn mv(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        self.txn.mv(self.absolute(src)?, self.absolute(dst)?)
    }

    /// Deep-copies the subtree at `src` to `dst` (both relative to the root).
    pub fn cp(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        self.txn.cp(self.absolute(src)?, self.absolute(dst)?)
    }

    /// Opens a write accessor over the file at `path` (relative to the root).
    pub fn fetch_mut<A: AMut<'a>>(&self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        self.txn.fetch_mut(self.absolute(path)?)
    }

    /// Resolves `path` against this view's root into an absolute access path.
    fn absolute(&self, path: impl IntoArborPath) -> AdbResult<APath> {
        Ok(self.root.join(&path.into_arbor_path()?))
    }
}
