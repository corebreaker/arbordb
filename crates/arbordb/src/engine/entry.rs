//! An arbor-node's stored value: a tagged blob that is either a directory or a file.

use crate::error::{AdbError, AdbResult};

mod tag {
    /// Leading byte marking a directory blob.
    pub(super) const DIR: u8 = 0;
    /// Leading byte marking a file blob.
    pub(super) const FILE: u8 = 1;
}

/// Whether an a-node is a directory or a file — the filesystem-level kind, distinct
/// from a value's [`NodeKind`](crate::NodeKind).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum EntryKind {
    /// A directory: a `Name → AKey` map of children.
    Dir,
    /// A file: a stored [`Value`](crate::Value).
    File,
}

/// One child in a directory listing: a name paired with its a-node kind.
///
/// Yielded by `ls` for each direct child of a directory, in name order. It
/// carries no [`AKey`](crate::AKey) — a listing exposes names and kinds, not a-node
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
        .ok_or_else(|| AdbError::Corrupt("empty a-node entry".into()))?;

    let kind = match tag {
        tag::DIR => EntryKind::Dir,
        tag::FILE => EntryKind::File,
        other => return Err(AdbError::Corrupt(format!("unknown a-node entry tag {other}"))),
    };

    Ok(kind)
}

/// Splits an entry blob into its kind and payload (the directory or value blob).
pub(crate) fn entry_split(entry: &[u8]) -> AdbResult<(EntryKind, &[u8])> {
    let (tag, payload) = entry
        .split_first()
        .ok_or_else(|| AdbError::Corrupt("empty a-node entry".into()))?;

    let kind = match *tag {
        tag::DIR => EntryKind::Dir,
        tag::FILE => EntryKind::File,
        other => return Err(AdbError::Corrupt(format!("unknown a-node entry tag {other}"))),
    };

    Ok((kind, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_exposes_its_name_and_kind() {
        let entry = Entry::new(String::from("x"), EntryKind::File);

        assert_eq!(entry.name(), "x");
        assert_eq!(entry.kind(), EntryKind::File);
    }

    #[test]
    fn reading_the_kind_rejects_an_empty_or_unknown_tag() {
        assert!(get_entry_kind(&[]).is_err());
        assert!(get_entry_kind(&[42]).is_err());
        assert!(entry_split(&[]).is_err());
        assert!(entry_split(&[42, 1, 2]).is_err());
    }

    #[test]
    fn a_built_entry_splits_back_into_its_kind_and_payload() {
        let dir = dir_entry(&[9, 9]);
        let (dir_kind, dir_payload) = entry_split(&dir).unwrap();
        assert_eq!(dir_kind, EntryKind::Dir);
        assert_eq!(dir_payload, &[9, 9]);

        let file = file_entry(&[1, 2, 3]);
        let (file_kind, file_payload) = entry_split(&file).unwrap();
        assert_eq!(file_kind, EntryKind::File);
        assert_eq!(file_payload, &[1, 2, 3]);

        let one = file_entry(&[1]);
        assert_eq!(get_entry_kind(&one).unwrap(), EntryKind::File);
    }
}
