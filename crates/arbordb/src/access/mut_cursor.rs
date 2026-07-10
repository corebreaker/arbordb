//! A [`Writer`] that edits one stored file in place.
//!
//! Backs `WriteTxn::fetch_mut`. It is a thin, stateless handle — a borrow of the
//! transaction plus the file's access path — so an `Arc` of it is `Send + Sync`.
//! Each operation loads the file's current value, applies the change, and (for a
//! write) re-encodes and rewrites the whole blob: the accepted O(blob) cost of a
//! partial write (ArborDb's win is on reads). Sequential edits accumulate because
//! each reads the latest committed-in-txn state.

use super::{Reader, Writer};
use crate::{
    data::Scalar,
    error::{AdbError, AdbResult},
    node::NodeKind,
    path::{APath, VPath},
    txn::WriteTxn,
    value::Value,
};

/// A read/write cursor over one file, reloading and rewriting it per operation.
pub(crate) struct MutCursor<'t> {
    txn:   &'t WriteTxn,
    apath: APath,
}

impl<'t> MutCursor<'t> {
    /// Opens a cursor over the file at `apath`.
    pub(crate) fn open(txn: &'t WriteTxn, apath: APath) -> Self {
        Self {
            txn,
            apath,
        }
    }

    /// The file's current value (an empty default if it has vanished).
    fn value(&self) -> AdbResult<Value> {
        Ok(self.txn.load_value_at(&self.apath)?.unwrap_or_default())
    }

    /// Re-encodes `value` and writes it back to the file.
    fn write(&self, value: &Value) -> AdbResult<()> {
        self.txn.store_value_at(&self.apath, value)
    }
}

impl Reader for MutCursor<'_> {
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>> {
        Ok(self.value()?.subtree(at).and_then(|node| node.leaf().cloned()))
    }

    fn kind_at(&self, at: &VPath) -> AdbResult<Option<NodeKind>> {
        Ok(self.value()?.subtree(at).map(Value::node_kind))
    }

    fn len_at(&self, at: &VPath) -> AdbResult<usize> {
        match self.value()?.subtree(at) {
            Some(Value::List(list)) => Ok(list.len()),
            Some(_) => Err(AdbError::CannotAccess(format!("'{at}' is not a list"))),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn keys_at(&self, at: &VPath) -> AdbResult<Vec<String>> {
        match self.value()?.subtree(at) {
            Some(Value::Node(map)) => Ok(map.keys().cloned().collect()),
            Some(_) => Err(AdbError::CannotAccess(format!("'{at}' is not an object"))),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn exists_at(&self, at: &VPath) -> AdbResult<bool> {
        Ok(self.value()?.subtree(at).is_some())
    }
}

impl Writer for MutCursor<'_> {
    fn put_scalar(&self, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        let mut value = self.value()?;
        value.set_value(at, Value::Leaf(scalar));

        self.write(&value)
    }

    fn ensure_container(&self, at: &VPath, list: bool) -> AdbResult<()> {
        let mut value = self.value()?;
        let matches = match value.subtree(at) {
            Some(Value::List(_)) => list,
            Some(Value::Node(_)) => !list,
            _ => false,
        };

        if !matches {
            let empty = if list {
                Value::new_empty_list()
            } else {
                Value::new_empty_node()
            };

            value.set_value(at, empty);
        }

        self.write(&value)
    }

    fn remove(&self, at: &VPath) -> AdbResult<bool> {
        let mut value = self.value()?;
        let removed = value.remove_value(at);
        self.write(&value)?;

        Ok(removed)
    }
}
