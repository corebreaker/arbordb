//! A node's stored value: a tagged blob that is either a directory or a file.

use crate::error::{AdbError, AdbResult};

mod tag {
    pub(super) const DIR: u8 = 0;
    pub(super) const FILE: u8 = 1;
}

/// Whether a node is a directory or a file — the filesystem-level kind, distinct
/// from a value's [`NodeKind`](crate::NodeKind).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    /// A directory: a `Name → AKey` map of children.
    Dir,
    /// A file: a stored [`Value`](crate::Value).
    File,
}

/// Builds a directory entry from an encoded directory blob.
pub(crate) fn dir_entry(dir_blob: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + dir_blob.len());
    out.push(tag::DIR);
    out.extend_from_slice(dir_blob);

    out
}

/// Builds a file entry from an encoded value blob.
pub(crate) fn file_entry(value_blob: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + value_blob.len());
    out.push(tag::FILE);
    out.extend_from_slice(value_blob);

    out
}

/// Splits an entry blob into its kind and payload (the directory or value blob).
pub(crate) fn split(entry: &[u8]) -> AdbResult<(EntryKind, &[u8])> {
    let (tag, payload) = entry
        .split_first()
        .ok_or_else(|| AdbError::Corrupt("empty node entry".into()))?;

    let kind = match *tag {
        tag::DIR => EntryKind::Dir,
        tag::FILE => EntryKind::File,
        other => return Err(AdbError::Corrupt(format!("unknown node entry tag {other}"))),
    };

    Ok((kind, payload))
}
