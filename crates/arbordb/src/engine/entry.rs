//! A vnode's stored value: a tagged blob that is either a directory or a file.

use crate::error::{AdbError, AdbResult};

mod tag {
    /// Leading byte marking a directory blob.
    pub(super) const DIR: u8 = 0;
    /// Leading byte marking a file blob.
    pub(super) const FILE: u8 = 1;
}

/// Whether a vnode is a directory or a file — the filesystem-level kind, distinct
/// from a value's [`NodeKind`](crate::NodeKind).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum EntryKind {
    /// A directory: a `Name → AKey` map of children.
    Dir,
    /// A file: a stored [`Value`](crate::Value).
    File,
}

/// One child in a directory listing: a name paired with its vnode kind.
///
/// Yielded by `ls` for each direct child of a directory, in name order. It
/// carries no [`AKey`](crate::AKey) — a listing exposes names and kinds, not vnode
/// identities.
#[derive(Clone, PartialEq, Eq, Debug, Hash)]
pub struct Entry {
    /// The child's name within its parent directory.
    name: String,
    /// Whether the child is a directory or a file.
    kind: EntryKind,
}

impl Entry {
    /// Pairs a child `name` with its `kind`.
    pub fn new(name: String, kind: EntryKind) -> Self {
        Self {
            kind,
            name,
        }
    }

    /// The child's name within its parent directory.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the child is a directory or a file.
    pub fn kind(&self) -> EntryKind {
        self.kind
    }
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

/// The kind of an entry blob, read from its leading tag byte alone. Use
/// [`entry_split`] instead when the payload is also needed.
pub(crate) fn get_entry_kind(entry: &[u8]) -> AdbResult<EntryKind> {
    let tag = entry
        .first()
        .copied()
        .ok_or_else(|| AdbError::Corrupt("empty vnode entry".into()))?;

    let kind = match tag {
        tag::DIR => EntryKind::Dir,
        tag::FILE => EntryKind::File,
        other => return Err(AdbError::Corrupt(format!("unknown vnode entry tag {other}"))),
    };

    Ok(kind)
}

/// Splits an entry blob into its kind and payload (the directory or value blob).
pub(crate) fn entry_split(entry: &[u8]) -> AdbResult<(EntryKind, &[u8])> {
    let (tag, payload) = entry
        .split_first()
        .ok_or_else(|| AdbError::Corrupt("empty vnode entry".into()))?;

    let kind = match *tag {
        tag::DIR => EntryKind::Dir,
        tag::FILE => EntryKind::File,
        other => return Err(AdbError::Corrupt(format!("unknown vnode entry tag {other}"))),
    };

    Ok((kind, payload))
}
