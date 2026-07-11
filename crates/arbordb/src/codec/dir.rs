//! The directory blob format: a name-sorted `Name → AKey` map of a directory's
//! children, navigated zero-copy by binary search — the filesystem-level mirror
//! of the value codec's object vnode.
//!
//! Layout: a `count` (`u32`), then that many fixed-width entries
//! `(name offset, name length, child key)`, then the name bytes. Entries are kept
//! in name order (a [`BTreeMap`] supplies that), so a child is found without
//! decoding the whole map.

use super::{
    put_u32,
    read::{read_u32, slice},
};

use crate::{
    error::{AdbError, AdbResult},
    AKey,
};

use smol_str::SmolStr;
use std::{cmp::Ordering, collections::BTreeMap};

/// Bytes per entry: name offset (`u32`) + name length (`u32`) + child key (16).
const ENTRY: usize = 4 + 4 + 16;

/// Serialises a directory's `Name → AKey` children into a blob. Names are held as
/// [`SmolStr`], which stores a short child name (the common case) inline, so building
/// a directory's child-map allocates nothing per name.
pub(crate) fn encode_dir(children: &BTreeMap<SmolStr, AKey>) -> Vec<u8> {
    let count = children.len();
    let names_start = 4 + count * ENTRY;

    let mut out = Vec::with_capacity(names_start);
    let mut names = Vec::new();
    put_u32(&mut out, count as u32);
    for (name, akey) in children {
        // `BTreeMap` iterates in name order, so the entry table is name-sorted.
        let name_off = (names_start + names.len()) as u32;
        put_u32(&mut out, name_off);
        put_u32(&mut out, name.len() as u32);
        out.extend_from_slice(&akey.into_bytes());
        names.extend_from_slice(name.as_bytes());
    }

    out.extend_from_slice(&names);

    out
}

/// A validated, zero-copy view over a directory blob.
pub(crate) struct ArchivedDir<'a> {
    /// The borrowed directory blob.
    blob: &'a [u8],
}

impl<'a> ArchivedDir<'a> {
    /// Wraps `blob`, checking only that its `count` header is present; deeper
    /// reads are bounds-checked as they happen.
    pub(crate) fn new(blob: &'a [u8]) -> AdbResult<Self> {
        read_u32(blob, 0)?;

        Ok(Self {
            blob,
        })
    }

    /// The number of children.
    pub(crate) fn len(&self) -> AdbResult<usize> {
        Ok(read_u32(self.blob, 0)? as usize)
    }

    /// The child key named `name`, found by binary search over the entry table.
    pub(crate) fn get(&self, name: &str) -> AdbResult<Option<AKey>> {
        let count = self.len()?;

        let needle = name.as_bytes();
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry = 4 + mid * ENTRY;

            // Compare the raw name bytes: the table is byte-sorted and names were
            // validated as UTF-8 at write time, so the per-comparison `from_utf8`
            // check is unnecessary here.
            match self.entry_name_bytes(entry)?.cmp(needle) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(Some(self.entry_akey(entry)?)),
            }
        }

        Ok(None)
    }

    /// Every `(child name, child key)`, in name order.
    pub(crate) fn entries(&self) -> AdbResult<Vec<(&'a str, AKey)>> {
        let count = self.len()?;

        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let entry = 4 + i * ENTRY;
            out.push((self.entry_name(entry)?, self.entry_akey(entry)?));
        }

        Ok(out)
    }

    /// The raw name bytes of the entry whose record starts at `entry`. Used by the
    /// binary search, which orders by bytes and needs no UTF-8 check.
    fn entry_name_bytes(&self, entry: usize) -> AdbResult<&'a [u8]> {
        let name_off = read_u32(self.blob, entry)? as usize;
        let name_len = read_u32(self.blob, entry + 4)? as usize;

        slice(self.blob, name_off, name_len)
    }

    /// The child name of the entry whose record starts at `entry`.
    fn entry_name(&self, entry: usize) -> AdbResult<&'a str> {
        let bytes = self.entry_name_bytes(entry)?;

        std::str::from_utf8(bytes).map_err(|_| AdbError::Corrupt("invalid utf-8 in a directory name".into()))
    }

    /// The child key of the entry whose record starts at `entry`.
    fn entry_akey(&self, entry: usize) -> AdbResult<AKey> {
        let bytes = slice(self.blob, entry + 8, 16)?;

        Ok(AKey::from_bytes(bytes.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(pairs: &[(&str, u128)]) -> BTreeMap<SmolStr, AKey> {
        pairs.iter().map(|(n, k)| (SmolStr::from(*n), AKey::from(*k))).collect()
    }

    #[test]
    fn get_finds_children_and_misses() {
        let children = dir(&[("alice", 1), ("bob", 2), ("carol", 3)]);
        let blob = encode_dir(&children);
        let view = ArchivedDir::new(&blob).unwrap();

        assert_eq!(view.len().unwrap(), 3);
        assert_eq!(view.get("alice").unwrap(), Some(AKey::from(1)));
        assert_eq!(view.get("bob").unwrap(), Some(AKey::from(2)));
        assert_eq!(view.get("carol").unwrap(), Some(AKey::from(3)));
        assert_eq!(view.get("dave").unwrap(), None);
        assert_eq!(view.get("").unwrap(), None);
    }

    #[test]
    fn entries_come_back_name_sorted() {
        let children = dir(&[("zoe", 9), ("amy", 1), ("mia", 5)]);
        let blob = encode_dir(&children);
        let view = ArchivedDir::new(&blob).unwrap();

        let names: Vec<&str> = view.entries().unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["amy", "mia", "zoe"]);
    }

    #[test]
    fn an_empty_dir_roundtrips() {
        let blob = encode_dir(&BTreeMap::new());
        let view = ArchivedDir::new(&blob).unwrap();

        assert_eq!(view.len().unwrap(), 0);
        assert_eq!(view.get("anything").unwrap(), None);
        assert!(view.entries().unwrap().is_empty());
    }
}
