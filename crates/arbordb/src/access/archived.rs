//! A [`Reader`] over a stored value blob, navigated zero-copy.
//!
//! Holds the file's whole entry behind an [`Arc`] (shared with the transaction's
//! blob cache) plus the offset at which the value payload begins (just past the
//! entry tag), and re-wraps that payload as an [`ArchivedValue`] per call —
//! reading a field straight out of the bytes without materializing the rest.

use super::Reader;
use crate::{
    codec::ArchivedValue,
    data::Scalar,
    error::{AdbError, AdbResult},
    node::NodeKind,
    path::VPath,
};

use std::sync::Arc;

/// A read cursor over one stored file's value blob.
pub(crate) struct ArchivedReader {
    entry:  Arc<Vec<u8>>,
    offset: usize,
}

impl ArchivedReader {
    /// Wraps a file entry, reading its value payload starting at `offset` (past
    /// the entry tag).
    pub(crate) fn new(entry: Arc<Vec<u8>>, offset: usize) -> Self {
        Self {
            entry,
            offset,
        }
    }

    /// The value payload as a validated [`ArchivedValue`].
    fn view(&self) -> AdbResult<ArchivedValue<'_>> {
        let payload = self
            .entry
            .get(self.offset..)
            .ok_or_else(|| AdbError::Corrupt("truncated file entry".into()))?;

        ArchivedValue::new(payload)
    }
}

impl Reader for ArchivedReader {
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>> {
        let view = self.view()?;
        match view.root().navigate(at)? {
            Some(node) if matches!(node.kind()?, NodeKind::Leaf) => Ok(Some(node.scalar()?)),
            _ => Ok(None),
        }
    }

    fn kind_at(&self, at: &VPath) -> AdbResult<Option<NodeKind>> {
        let view = self.view()?;
        match view.root().navigate(at)? {
            Some(node) => Ok(Some(node.kind()?)),
            None => Ok(None),
        }
    }

    fn len_at(&self, at: &VPath) -> AdbResult<usize> {
        let view = self.view()?;
        match view.root().navigate(at)? {
            Some(node) => node.len(),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn keys_at(&self, at: &VPath) -> AdbResult<Vec<String>> {
        let view = self.view()?;
        match view.root().navigate(at)? {
            Some(node) => node.object_keys(),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn exists_at(&self, at: &VPath) -> AdbResult<bool> {
        let view = self.view()?;

        Ok(view.root().navigate(at)?.is_some())
    }
}
