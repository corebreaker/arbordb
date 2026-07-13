//! ArborDb's own zero-copy value format — its answer to rkyv, specialised for a
//! [`Value`] and free of any alignment requirement.
//!
//! [`encode`] serialises a `Value` into one self-contained blob laid out with
//! **offset tables**, so any field or element is reached by following an offset
//! rather than by decoding what comes before it:
//!
//! - a **leaf** is the scalar's own byte codec ([`Scalar::encode`]);
//! - a **list** is a `count` followed by that many child offsets (an O(1) jump to element `i`);
//! - an **object** is a `count` followed by that many `(name, name len, child)` entries, kept name-sorted so a field is
//!   found by binary search.
//!
//! Every offset is absolute from the blob start, so the tree is laid out
//! children-first with the root last; a 5-byte header (`version` + root offset)
//! opens the blob. An [`ArchivedValue`] borrows the blob bytes directly — the redb
//! page, on the read path — and navigates them in place: every multi-byte read is
//! an explicit `from_be_bytes` over an unaligned slice, so nothing needs copying
//! into an aligned buffer first (rkyv's cost that this format avoids).

use crate::{
    codec::{
        self,
        read::{read_u8, read_u32, slice},
        Reader,
    },
    data::Scalar,
    error::{AdbError, AdbResult},
    vnode::NodeKind,
    path::{Segment, VPath},
    value::Value,
};

use std::{cmp::Ordering, collections::BTreeMap};

/// The value-blob format version, recorded in the header's first byte.
const FORMAT_VERSION: u8 = 1;

/// The header length: `version` (1 byte) + root offset (`u32`, 4 bytes).
const HEADER_LEN: usize = 5;

/// Node discriminants, the first byte of every vnode.
const LEAF: u8 = 0;
const LIST: u8 = 1;
const NODE: u8 = 2;

/// Serialises `value` into a self-contained value blob.
pub(crate) fn encode(value: &Value) -> Vec<u8> {
    let mut buf = begin_blob();
    let root = push_value(value, &mut buf);
    patch_root(&mut buf, root);

    buf
}

/// Decodes a whole blob back into an owned [`Value`].
pub(crate) fn decode(blob: &[u8]) -> AdbResult<Value> {
    ArchivedValue::new(blob)?.to_value()
}

/// Appends `value`'s subtree to `buf` (children first) and returns the absolute
/// offset of the vnode's own header byte. Shared by [`encode`] and by the typed
/// direct encoder's default path ([`AData::encode_node`](crate::data::AData::encode_node)).
pub(crate) fn push_value(value: &Value, buf: &mut Vec<u8>) -> u32 {
    match value {
        Value::Leaf(scalar) => push_leaf(buf, scalar),
        Value::List(items) => {
            let child_offsets: Vec<u32> = items.iter().map(|item| push_value(item, buf)).collect();

            push_list(buf, &child_offsets)
        }
        Value::Node(map) => {
            // A `BTreeMap` iterates name-sorted, exactly what `push_object` requires.
            let mut entries: Vec<(&str, u32)> = Vec::with_capacity(map.len());
            for (name, child) in map {
                let child_off = push_value(child, buf);
                entries.push((name.as_str(), child_off));
            }

            push_object(buf, &entries)
        }
    }
}

/// Begins a value blob: the version byte plus a placeholder for the root offset,
/// which [`patch_root`] fills in once the root vnode has been written last.
pub(crate) fn begin_blob() -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN);
    buf.push(FORMAT_VERSION);
    buf.extend_from_slice(&0u32.to_be_bytes());

    buf
}

/// Back-patches the header's root offset once the root vnode sits at `root`.
pub(crate) fn patch_root(buf: &mut [u8], root: u32) {
    buf[1..HEADER_LEN].copy_from_slice(&root.to_be_bytes());
}

/// Appends a leaf vnode holding `scalar`, returning its absolute offset.
pub(crate) fn push_leaf(buf: &mut Vec<u8>, scalar: &Scalar) -> u32 {
    let off = offset(buf);
    buf.push(LEAF);
    scalar.encode(buf);

    off
}

/// Appends a list vnode over `children` (their already-written offsets), returning
/// its absolute offset.
pub(crate) fn push_list(buf: &mut Vec<u8>, children: &[u32]) -> u32 {
    let off = offset(buf);
    buf.push(LIST);
    codec::put_u32(buf, children.len() as u32);
    for child in children {
        codec::put_u32(buf, *child);
    }

    off
}

/// Appends an object vnode over `entries` — `(field name, already-written child
/// offset)` pairs that MUST be name-sorted and unique, since lookups binary-search
/// the table. Writes the field-name bytes and then the entry table, returning the
/// object's absolute offset.
pub(crate) fn push_object(buf: &mut Vec<u8>, entries: &[(&str, u32)]) -> u32 {
    let mut records: Vec<(u32, u32, u32)> = Vec::with_capacity(entries.len());
    for (name, child_off) in entries {
        let name_off = offset(buf);
        buf.extend_from_slice(name.as_bytes());
        records.push((name_off, name.len() as u32, *child_off));
    }

    let off = offset(buf);
    buf.push(NODE);
    codec::put_u32(buf, records.len() as u32);
    for (name_off, name_len, child_off) in records {
        codec::put_u32(buf, name_off);
        codec::put_u32(buf, name_len);
        codec::put_u32(buf, child_off);
    }

    off
}

/// The current write position as a `u32` offset (values stay far below 4 GiB).
fn offset(buf: &[u8]) -> u32 {
    debug_assert!(
        buf.len() <= u32::MAX as usize,
        "value blob exceeds the 4 GiB offset space"
    );

    buf.len() as u32
}

/// A validated, zero-copy view over a value blob.
///
/// Holds a borrow of the blob (the redb page, on a read) and the offset of its
/// root vnode. Navigation reads happen straight out of the borrowed bytes.
pub(crate) struct ArchivedValue<'a> {
    /// The borrowed value blob (a redb page, on a read).
    blob: &'a [u8],
    /// Offset of the root vnode within `blob`.
    root: u32,
}

impl<'a> ArchivedValue<'a> {
    /// Validates the header of `blob` and captures its root offset.
    pub(crate) fn new(blob: &'a [u8]) -> AdbResult<Self> {
        let version = *blob
            .first()
            .ok_or_else(|| AdbError::Corrupt("empty value blob".into()))?;

        if version != FORMAT_VERSION {
            return Err(AdbError::SchemaMismatch(format!(
                "value blob format version {version}, expected {FORMAT_VERSION}"
            )));
        }

        let root = read_u32(blob, 1)?;

        Ok(Self {
            blob,
            root,
        })
    }

    /// Re-wraps `blob` at an already-validated `root` offset, skipping the header
    /// re-check — for a cursor (an [`ArchivedReader`](crate::access::ArchivedReader))
    /// that validated the header once at construction and re-views the same blob on
    /// every field read.
    pub(crate) fn with_root(blob: &'a [u8], root: u32) -> Self {
        Self {
            blob,
            root,
        }
    }

    /// The captured root-vnode offset, so a caller can memoise a validated view and
    /// rebuild it with [`with_root`](Self::with_root) without re-reading the header.
    pub(crate) fn root_offset(&self) -> u32 {
        self.root
    }

    /// The root vnode.
    pub(crate) fn root(&self) -> ArchivedNode<'a> {
        ArchivedNode {
            blob: self.blob,
            off:  self.root,
        }
    }

    /// Materialises the whole blob into an owned [`Value`].
    pub(crate) fn to_value(&self) -> AdbResult<Value> {
        self.root().to_value()
    }

    /// If `at` lands on a leaf, the byte range `(offset, len)` of its encoded scalar
    /// within the blob — the span a same-width overwrite may patch in place. `None`
    /// if the path is absent or does not land on a leaf.
    pub(crate) fn leaf_scalar_span(&self, at: &VPath) -> AdbResult<Option<(usize, usize)>> {
        match self.root().navigate(at)? {
            Some(node) => node.leaf_scalar_span(),
            None => Ok(None),
        }
    }
}

/// A cursor at one vnode inside a value blob, navigated zero-copy.
#[derive(Clone, Copy)]
pub(crate) struct ArchivedNode<'a> {
    /// The borrowed value blob this cursor reads from.
    blob: &'a [u8],
    /// Offset of this vnode within `blob`.
    off:  u32,
}

impl<'a> ArchivedNode<'a> {
    /// The kind of this vnode.
    pub(crate) fn kind(&self) -> AdbResult<NodeKind> {
        match read_u8(self.blob, self.off as usize)? {
            LEAF => Ok(NodeKind::Leaf),
            LIST => Ok(NodeKind::List),
            NODE => Ok(NodeKind::Object),
            // no-coverage:start — a bad tag byte means a corrupted blob, not a normal path
            other => Err(AdbError::Corrupt(format!("unknown value vnode tag {other}"))),
            // no-coverage:stop
        }
    }

    /// Decodes this leaf's scalar (materialising strings/bytes).
    pub(crate) fn scalar(&self) -> AdbResult<Scalar> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != LEAF {
            // no-coverage:start — callers check the kind first; this guards a codec misuse
            return Err(AdbError::Corrupt("a scalar was read from a non-leaf vnode".into()));
            // no-coverage:stop
        }

        let body = self
            .blob
            .get(off + 1..)
            .ok_or_else(|| AdbError::Corrupt("value blob is truncated".into()))?;

        Scalar::decode(&mut Reader::new(body))
    }

    /// This leaf's scalar if it is a leaf, or `None` otherwise — reading the
    /// discriminant byte **once**, unlike a `kind()` then `scalar()` pair. The hot
    /// scalar read (`scalar_at`, `get`) goes through this.
    pub(crate) fn scalar_if_leaf(&self) -> AdbResult<Option<Scalar>> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != LEAF {
            return Ok(None);
        }

        let body = self
            .blob
            .get(off + 1..)
            .ok_or_else(|| AdbError::Corrupt("value blob is truncated".into()))?;

        Ok(Some(Scalar::decode(&mut Reader::new(body))?))
    }

    /// If this is a leaf, the byte range `(offset, len)` of its encoded scalar within
    /// the blob — the bytes right after the leaf discriminant. `None` for a non-leaf.
    ///
    /// The length is measured by decoding the scalar, so it holds for both the
    /// fixed-width scalars and the length-prefixed ones (strings, bytes, bignums).
    fn leaf_scalar_span(&self) -> AdbResult<Option<(usize, usize)>> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != LEAF {
            return Ok(None);
        }

        let scalar_off = off + 1;
        let body = self
            .blob
            .get(scalar_off..)
            .ok_or_else(|| AdbError::Corrupt("value blob is truncated".into()))?;

        let mut reader = Reader::new(body);
        Scalar::decode(&mut reader)?;

        Ok(Some((scalar_off, reader.position())))
    }

    /// The number of elements, if this is a list vnode.
    pub(crate) fn len(&self) -> AdbResult<usize> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != LIST {
            return Err(AdbError::Corrupt("the length of a non-list vnode was requested".into()));
        }

        Ok(read_u32(self.blob, off + 1)? as usize)
    }

    /// The list element at `index`, or `None` if this is not a list or `index` is
    /// out of range.
    pub(crate) fn at(&self, index: usize) -> AdbResult<Option<ArchivedNode<'a>>> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != LIST {
            return Ok(None);
        }

        let count = read_u32(self.blob, off + 1)? as usize;
        if index >= count {
            return Ok(None);
        }

        let child = read_u32(self.blob, off + 5 + index * 4)?;

        Ok(Some(ArchivedNode {
            blob: self.blob,
            off:  child,
        }))
    }

    /// The object field named `name`, or `None` if this is not an object or the
    /// field is absent. Found by binary search over the name-sorted entry table.
    pub(crate) fn get(&self, name: &str) -> AdbResult<Option<ArchivedNode<'a>>> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != NODE {
            return Ok(None);
        }

        let count = read_u32(self.blob, off + 1)? as usize;
        let table = off + 5;

        let needle = name.as_bytes();
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry = table + mid * 12;
            // Compare the raw name bytes: the table is byte-sorted and names were
            // validated as UTF-8 at write time, so skipping the per-comparison
            // `from_utf8` check is both correct and faster.
            let entry_name = self.entry_name_bytes(entry)?;

            match entry_name.cmp(needle) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => {
                    let child = read_u32(self.blob, entry + 8)?;

                    return Ok(Some(ArchivedNode {
                        blob: self.blob,
                        off:  child,
                    }));
                }
            }
        }

        Ok(None)
    }

    /// Every `(field name, child vnode)` of this object, in name order.
    pub(crate) fn entries(&self) -> AdbResult<Vec<(&'a str, ArchivedNode<'a>)>> {
        let off = self.off as usize;
        if read_u8(self.blob, off)? != NODE {
            return Err(AdbError::Corrupt(
                "the entries of a non-object vnode were requested".into(),
            ));
        }

        let count = read_u32(self.blob, off + 1)? as usize;
        let table = off + 5;

        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let entry = table + i * 12;
            let name = self.entry_name(entry)?;
            let child = read_u32(self.blob, entry + 8)?;

            out.push((
                name,
                ArchivedNode {
                    blob: self.blob,
                    off:  child,
                },
            ));
        }

        Ok(out)
    }

    /// The raw name bytes of the object entry whose 12-byte record starts at `entry`.
    /// Used by the binary search, which orders by bytes and needs no UTF-8 check.
    fn entry_name_bytes(&self, entry: usize) -> AdbResult<&'a [u8]> {
        let name_off = read_u32(self.blob, entry)? as usize;
        let name_len = read_u32(self.blob, entry + 4)? as usize;

        slice(self.blob, name_off, name_len)
    }

    /// The field name of the object entry whose 12-byte record starts at `entry`.
    fn entry_name(&self, entry: usize) -> AdbResult<&'a str> {
        let bytes = self.entry_name_bytes(entry)?;

        std::str::from_utf8(bytes).map_err(|_| AdbError::Corrupt("invalid utf-8 in a field name".into()))
    }

    /// Follows a [`VPath`] from this vnode, returning the vnode it lands on, or
    /// `None` if a segment leads nowhere.
    pub(crate) fn navigate(self, at: &VPath) -> AdbResult<Option<ArchivedNode<'a>>> {
        let mut node = self;
        for segment in at.segments() {
            let next = match segment {
                Segment::Name(name) => node.get(name.as_str())?,
                Segment::Index(index) => node.at(*index as usize)?,
            };

            match next {
                Some(child) => node = child,
                None => return Ok(None),
            }
        }

        Ok(Some(node))
    }

    /// The field names of this object vnode, in name order.
    pub(crate) fn object_keys(&self) -> AdbResult<Vec<String>> {
        Ok(self.entries()?.into_iter().map(|(name, _)| name.to_string()).collect())
    }

    /// Materialises this vnode's subtree into an owned [`Value`].
    pub(crate) fn to_value(self) -> AdbResult<Value> {
        let value = match self.kind()? {
            NodeKind::Leaf => Value::Leaf(self.scalar()?),
            NodeKind::List => {
                let count = self.len()?;
                let mut items = Vec::with_capacity(count);
                for i in 0..count {
                    let child = self
                        .at(i)?
                        .ok_or_else(|| AdbError::Corrupt("list element missing".into()))?;

                    items.push(child.to_value()?);
                }

                Value::List(items)
            }
            NodeKind::Object => {
                let mut map = BTreeMap::new();
                for (name, child) in self.entries()? {
                    map.insert(name.to_string(), child.to_value()?);
                }

                Value::Node(map)
            }
        };

        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: Value) {
        let blob = encode(&value);

        assert_eq!(decode(&blob).unwrap(), value);
    }

    fn node(pairs: &[(&str, Value)]) -> Value {
        Value::Node(pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect())
    }

    #[test]
    fn scalars_roundtrip() {
        roundtrip(Value::Leaf(Scalar::Null));
        roundtrip(Value::Leaf(Scalar::I64(-42)));
        roundtrip(Value::Leaf(Scalar::Str(String::from("hello"))));
        roundtrip(Value::Leaf(Scalar::Bytes(vec![0, 1, 2, 255])));
    }

    #[test]
    fn lists_and_objects_roundtrip() {
        roundtrip(Value::new_empty_list());
        roundtrip(Value::new_empty_node());
        roundtrip(Value::new_list(vec![
            Value::Leaf(Scalar::U8(1)),
            Value::Leaf(Scalar::U8(2)),
        ]));
        roundtrip(node(&[
            ("age", Value::Leaf(Scalar::U32(30))),
            ("name", Value::Leaf(Scalar::Str(String::from("alice")))),
        ]));
    }

    #[test]
    fn nested_shapes_roundtrip() {
        let value = node(&[
            (
                "scores",
                Value::new_list(vec![Value::Leaf(Scalar::I64(10)), Value::Leaf(Scalar::I64(20))]),
            ),
            (
                "who",
                node(&[
                    ("first", Value::Leaf(Scalar::Str(String::from("a")))),
                    ("last", Value::Leaf(Scalar::Str(String::from("b")))),
                ]),
            ),
        ]);

        roundtrip(value);
    }

    #[test]
    fn navigation_reaches_fields_and_elements_zero_copy() {
        let value = node(&[
            (
                "who",
                node(&[("name", Value::Leaf(Scalar::Str(String::from("alice"))))]),
            ),
            (
                "scores",
                Value::new_list(vec![
                    Value::Leaf(Scalar::I64(10)),
                    Value::Leaf(Scalar::I64(20)),
                    Value::Leaf(Scalar::I64(30)),
                ]),
            ),
        ]);

        let blob = encode(&value);
        let view = ArchivedValue::new(&blob).unwrap();
        let root = view.root();

        assert_eq!(root.kind().unwrap(), NodeKind::Object);

        let name = root.get("who").unwrap().unwrap().get("name").unwrap().unwrap();
        assert_eq!(name.scalar().unwrap(), Scalar::Str(String::from("alice")));

        let scores = root.get("scores").unwrap().unwrap();
        assert_eq!(scores.kind().unwrap(), NodeKind::List);
        assert_eq!(scores.len().unwrap(), 3);
        assert_eq!(scores.at(1).unwrap().unwrap().scalar().unwrap(), Scalar::I64(20));

        // Absent field and out-of-range element resolve to `None`.
        assert!(root.get("missing").unwrap().is_none());
        assert!(scores.at(9).unwrap().is_none());
    }

    #[test]
    fn a_bad_version_is_rejected() {
        let mut blob = encode(&Value::Leaf(Scalar::Null));
        blob[0] = 0xFF;

        assert!(matches!(ArchivedValue::new(&blob), Err(AdbError::SchemaMismatch(_))));
    }

    #[test]
    fn a_truncated_blob_is_rejected() {
        let blob = encode(&node(&[("a", Value::Leaf(Scalar::I64(1)))]));

        assert!(decode(&blob[..blob.len() - 1]).is_err());
    }
}
