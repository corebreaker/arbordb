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
    vnode::NodeKind,
    path::VPath,
};

use std::sync::Arc;

/// A read cursor over one stored file's value blob.
pub(crate) struct ArchivedReader {
    /// The file's whole entry blob, shared with the transaction's blob cache.
    entry:  Arc<Vec<u8>>,
    /// Where the value payload begins inside `entry` (just past the entry tag).
    offset: usize,
    /// The value's root-vnode offset, captured once at construction so each field
    /// read re-views the blob without re-validating its header.
    root:   u32,
}

impl ArchivedReader {
    /// Wraps a file entry, reading its value payload starting at `offset` (past the
    /// entry tag). The value header is validated **once** here; every later field
    /// read rebuilds the view from the captured root offset without re-checking it.
    pub(crate) fn new(entry: Arc<Vec<u8>>, offset: usize) -> AdbResult<Self> {
        let payload = entry
            .get(offset..)
            .ok_or_else(|| AdbError::Corrupt("truncated file entry".into()))?;

        let root = ArchivedValue::new(payload)?.root_offset();

        Ok(Self {
            entry,
            offset,
            root,
        })
    }

    /// The value payload as an [`ArchivedValue`], rebuilt from the root offset
    /// captured at construction (the header was validated there, so this skips it).
    fn view(&self) -> AdbResult<ArchivedValue<'_>> {
        let payload = self
            .entry
            .get(self.offset..)
            .ok_or_else(|| AdbError::Corrupt("truncated file entry".into()))?;

        Ok(ArchivedValue::with_root(payload, self.root))
    }
}

impl Reader for ArchivedReader {
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>> {
        let view = self.view()?;
        match view.root().navigate(at)? {
            Some(node) => node.scalar_if_leaf(),
            None => Ok(None),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codec, value::Value};

    use std::collections::BTreeMap;

    fn vp(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    fn reader(value: &Value) -> ArchivedReader {
        ArchivedReader::new(Arc::new(codec::encode(value)), 0).unwrap()
    }

    #[test]
    fn archived_reader_navigates_a_stored_value() {
        let value = Value::Node(BTreeMap::from([
            (String::from("n"), Value::Leaf(Scalar::I64(5))),
            (
                String::from("xs"),
                Value::List(vec![Value::Leaf(Scalar::I64(1)), Value::Leaf(Scalar::I64(2))]),
            ),
        ]));

        let r = reader(&value);

        assert_eq!(r.scalar_at(&vp("n")).unwrap(), Some(Scalar::I64(5)));
        assert_eq!(r.kind_at(&vp("n")).unwrap(), Some(NodeKind::Leaf));
        assert_eq!(r.kind_at(&vp("xs")).unwrap(), Some(NodeKind::List));
        assert_eq!(r.kind_at(&VPath::root()).unwrap(), Some(NodeKind::Object));
        assert_eq!(r.len_at(&vp("xs")).unwrap(), 2);
        assert_eq!(
            r.keys_at(&VPath::root()).unwrap(),
            vec![String::from("n"), String::from("xs")]
        );
        assert!(r.exists_at(&vp("n")).unwrap());

        // A path that leads nowhere: scalar/kind report `None`, length/keys error.
        let missing = vp("nope");
        assert!(r.scalar_at(&missing).unwrap().is_none());
        assert!(r.kind_at(&missing).unwrap().is_none());
        assert!(!r.exists_at(&missing).unwrap());
        assert!(r.len_at(&missing).is_err());
        assert!(r.keys_at(&missing).is_err());

        // A truncated offset past the entry is rejected at construction.
        assert!(ArchivedReader::new(Arc::new(codec::encode(&value)), 9999).is_err());
    }
}
