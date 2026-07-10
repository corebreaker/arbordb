//! A [`Writer`] that builds a value in memory, used by `store`.
//!
//! `AData::store` drives this to assemble an owned [`Value`]; the transaction then
//! encodes the finished value into one blob. Interior mutability (`RefCell`) lets
//! the `Writer` methods take `&self`, matching the shared-cursor shape.

use super::{Reader, Writer};
use crate::{
    data::Scalar,
    error::{AdbError, AdbResult},
    vnode::NodeKind,
    path::VPath,
    value::Value,
};

use std::cell::RefCell;

/// A value being assembled in memory.
pub(crate) struct MemWriter {
    value: RefCell<Value>,
}

impl MemWriter {
    /// A fresh builder rooted at an empty object.
    pub(crate) fn new() -> Self {
        Self {
            value: RefCell::new(Value::new_empty_node()),
        }
    }

    /// Consumes the builder, yielding the assembled value.
    pub(crate) fn into_value(self) -> Value {
        self.value.into_inner()
    }
}

impl Reader for MemWriter {
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>> {
        Ok(self.value.borrow().subtree(at).and_then(|node| node.leaf().cloned()))
    }

    fn kind_at(&self, at: &VPath) -> AdbResult<Option<NodeKind>> {
        Ok(self.value.borrow().subtree(at).map(Value::node_kind))
    }

    fn len_at(&self, at: &VPath) -> AdbResult<usize> {
        match self.value.borrow().subtree(at) {
            Some(Value::List(list)) => Ok(list.len()),
            Some(_) => Err(AdbError::CannotAccess(format!("'{at}' is not a list"))),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn keys_at(&self, at: &VPath) -> AdbResult<Vec<String>> {
        match self.value.borrow().subtree(at) {
            Some(Value::Node(map)) => Ok(map.keys().cloned().collect()),
            Some(_) => Err(AdbError::CannotAccess(format!("'{at}' is not an object"))),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn exists_at(&self, at: &VPath) -> AdbResult<bool> {
        Ok(self.value.borrow().subtree(at).is_some())
    }
}

impl Writer for MemWriter {
    fn put_scalar(&self, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        self.value.borrow_mut().set_value(at, Value::Leaf(scalar));

        Ok(())
    }

    fn ensure_container(&self, at: &VPath, list: bool) -> AdbResult<()> {
        let mut value = self.value.borrow_mut();
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

        Ok(())
    }

    fn remove(&self, at: &VPath) -> AdbResult<bool> {
        Ok(self.value.borrow_mut().remove_value(at))
    }
}
