//! The typed **direct encoder**: building a value blob straight from an [`AData`]
//! value, without the intermediate [`Value`] tree.
//!
//! `WriteTxn::store` used to decompose a typed value into an in-memory `Value`
//! (a `BTreeMap`-backed tree, one heap key per field) and then walk that tree a
//! second time to encode it. [`encode_data`] instead drives the value's own
//! [`AData::encode_node`] to emit the blob in a single children-first pass — no
//! `Value`, no second walk. The scalars, containers, and simple derived structs
//! override `encode_node` to take this path; anything with a custom or flattened
//! store falls back to the (still correct) `Value`-based default.

use crate::{
    codec::{begin_blob, patch_root, push_leaf, push_list, push_object, push_value},
    data::{AData, Scalar},
    error::AdbResult,
    value::Value,
};

/// A sink that emits one value blob's vnodes children-first, driven by
/// [`AData::encode_node`]. Public only so the `#[derive(AData)]` macro's generated
/// code can drive it from the user's crate; it is **not** part of the stable API.
#[doc(hidden)]
pub struct NodeEncoder<'b> {
    /// The blob being built.
    buf: &'b mut Vec<u8>,
}

impl<'b> NodeEncoder<'b> {
    /// Wraps the buffer being built.
    pub(crate) fn new(buf: &'b mut Vec<u8>) -> Self {
        Self {
            buf,
        }
    }

    /// Emits a leaf vnode holding `scalar`, returning its offset.
    #[doc(hidden)]
    pub fn leaf(&mut self, scalar: &Scalar) -> u32 {
        push_leaf(self.buf, scalar)
    }

    /// Emits a list vnode over `children` (their already-emitted offsets, in order),
    /// returning its offset.
    #[doc(hidden)]
    pub fn list(&mut self, children: &[u32]) -> u32 {
        push_list(self.buf, children)
    }

    /// Emits an object vnode over `entries` — `(field name, already-emitted child
    /// offset)` pairs that MUST be name-sorted and unique — returning its offset.
    #[doc(hidden)]
    pub fn object(&mut self, entries: &[(&str, u32)]) -> u32 {
        push_object(self.buf, entries)
    }

    /// Emits an owned [`Value`] subtree — the fallback the default
    /// [`AData::encode_node`] takes for a type that has no direct override.
    pub(crate) fn value(&mut self, value: &Value) -> u32 {
        push_value(value, self.buf)
    }
}

/// Encodes `value` straight into a value blob, bypassing the intermediate [`Value`]
/// tree — the fast path behind `WriteTxn::store`.
pub(crate) fn encode_data<T: AData>(value: &T) -> AdbResult<Vec<u8>> {
    let mut buf = begin_blob();

    let root = {
        let mut enc = NodeEncoder::new(&mut buf);

        value.encode_node(&mut enc)?
    };

    patch_root(&mut buf, root);

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{access::MemWriter, codec::encode, data::Bytes, path::VPath};
    use std::collections::BTreeMap;

    /// The reference blob: the pre-direct-encoder path — decompose into a `Value`
    /// through `store`, then encode that tree.
    fn reference<T: AData>(value: &T) -> Vec<u8> {
        let writer = MemWriter::new();
        value.store(&writer, &VPath::root()).unwrap();

        encode(&writer.into_value())
    }

    /// Every `AData::encode_node` override MUST emit the byte-for-byte blob the
    /// `Value`-based default produces, so the direct path is a faithful replacement.
    fn assert_matches<T: AData>(value: &T) {
        assert_eq!(encode_data(value).unwrap(), reference(value));
    }

    #[test]
    fn direct_matches_the_value_path_for_scalars() {
        assert_matches(&42i64);
        assert_matches(&true);
        assert_matches(&String::from("hello"));
        assert_matches(&3.5f64);
        assert_matches(&7u8);
        assert_matches(&Bytes(vec![0, 1, 2, 255]));
    }

    #[test]
    fn direct_matches_the_value_path_for_options() {
        assert_matches(&Some(5i64));
        assert_matches(&Option::<i64>::None);
        assert_matches(&Some(String::from("x")));
    }

    #[test]
    fn direct_matches_the_value_path_for_lists() {
        assert_matches(&vec![1i64, 2, 3]);
        assert_matches(&Vec::<i64>::new());
        assert_matches(&vec![String::from("a"), String::from("bb")]);
    }

    #[test]
    fn direct_matches_the_value_path_for_maps_and_nesting() {
        let mut map: BTreeMap<String, i64> = BTreeMap::new();
        map.insert(String::from("zoe"), 9);
        map.insert(String::from("amy"), 1);
        map.insert(String::from("mia"), 5);
        assert_matches(&map);

        let mut nested: BTreeMap<String, Vec<i64>> = BTreeMap::new();
        nested.insert(String::from("evens"), vec![2, 4, 6]);
        nested.insert(String::from("odds"), vec![1, 3, 5]);
        assert_matches(&nested);

        let mut opts: BTreeMap<String, Option<String>> = BTreeMap::new();
        opts.insert(String::from("set"), Some(String::from("v")));
        opts.insert(String::from("unset"), None);
        assert_matches(&opts);
    }
}
