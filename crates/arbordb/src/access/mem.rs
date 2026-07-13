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
    /// The value under construction; `RefCell` lets the `Writer` methods take `&self`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn vp(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    /// Builds `{ n: 5, xs: [1] }` through the writer methods.
    fn built() -> MemWriter {
        let w = MemWriter::new();
        w.put_scalar(&vp("n"), Scalar::I64(5)).unwrap();
        w.ensure_container(&vp("xs"), true).unwrap();
        w.put_scalar(&vp("xs[0]"), Scalar::I64(1)).unwrap();

        w
    }

    #[test]
    fn mem_writer_reads_back_what_it_built() {
        let w = built();

        assert_eq!(w.scalar_at(&vp("n")).unwrap(), Some(Scalar::I64(5)));
        assert_eq!(w.kind_at(&vp("n")).unwrap(), Some(NodeKind::Leaf));
        assert_eq!(w.kind_at(&VPath::root()).unwrap(), Some(NodeKind::Object));
        assert_eq!(w.len_at(&vp("xs")).unwrap(), 1);
        assert_eq!(
            w.keys_at(&VPath::root()).unwrap(),
            vec![String::from("n"), String::from("xs")]
        );
        assert!(w.exists_at(&vp("n")).unwrap());
        assert!(!w.exists_at(&vp("missing")).unwrap());

        // A leaf has no scalar-less container; a container has no scalar.
        assert!(w.scalar_at(&vp("xs")).unwrap().is_none());

        // Error branches: length of a non-list, keys of a non-object, and both absent.
        assert!(w.len_at(&vp("n")).is_err());
        assert!(w.len_at(&vp("missing")).is_err());
        assert!(w.keys_at(&vp("n")).is_err());
        assert!(w.keys_at(&vp("missing")).is_err());

        // `ensure_container` is a no-op when the right kind already sits there.
        w.ensure_container(&vp("xs"), true).unwrap();
        w.ensure_container(&vp("obj"), false).unwrap();
        w.ensure_container(&vp("obj"), false).unwrap();
        assert_eq!(w.kind_at(&vp("obj")).unwrap(), Some(NodeKind::Object));

        assert!(w.remove(&vp("n")).unwrap());
        assert!(!w.exists_at(&vp("n")).unwrap());

        assert_eq!(built().into_value().get_value("n"), Some(Value::Leaf(Scalar::I64(5))));
    }

    // The production `Arc<dyn Writer>` wraps a `Send + Sync` cursor; here it wraps a
    // single-threaded `MemWriter` purely to exercise the forwarding impls in one place.
    #[allow(clippy::arc_with_non_send_sync)]
    #[test]
    fn boxed_and_arced_handles_forward_every_method() {
        // A `Box<dyn Writer>` forwards the Reader and Writer methods to the inner value.
        let boxed: Box<dyn Writer> = Box::new(built());
        boxed.put_scalar(&vp("m"), Scalar::I64(7)).unwrap();
        boxed.ensure_container(&vp("ys"), false).unwrap();
        assert_eq!(boxed.scalar_at(&vp("m")).unwrap(), Some(Scalar::I64(7)));
        assert_eq!(boxed.kind_at(&vp("m")).unwrap(), Some(NodeKind::Leaf));
        assert_eq!(boxed.len_at(&vp("xs")).unwrap(), 1);
        assert!(!boxed.keys_at(&VPath::root()).unwrap().is_empty());
        assert!(boxed.exists_at(&vp("m")).unwrap());
        assert!(boxed.remove(&vp("m")).unwrap());

        // An `Arc<dyn Writer>` forwards the same way (the accessor's shared shape).
        let arced: Arc<dyn Writer> = Arc::new(built());
        arced.put_scalar(&vp("z"), Scalar::I64(1)).unwrap();
        arced.ensure_container(&vp("zs"), true).unwrap();
        assert_eq!(arced.scalar_at(&vp("z")).unwrap(), Some(Scalar::I64(1)));
        assert_eq!(arced.kind_at(&vp("z")).unwrap(), Some(NodeKind::Leaf));
        assert_eq!(arced.len_at(&vp("zs")).unwrap(), 0);
        assert!(!arced.keys_at(&VPath::root()).unwrap().is_empty());
        assert!(arced.exists_at(&vp("z")).unwrap());
        assert!(arced.remove(&vp("z")).unwrap());

        // A `Box`/`Arc` of a bare `dyn Reader` forwards the read methods too.
        let boxed_reader: Box<dyn Reader> = Box::new(built());
        assert_eq!(boxed_reader.scalar_at(&vp("n")).unwrap(), Some(Scalar::I64(5)));
        assert_eq!(boxed_reader.kind_at(&vp("n")).unwrap(), Some(NodeKind::Leaf));
        assert_eq!(boxed_reader.len_at(&vp("xs")).unwrap(), 1);
        assert_eq!(boxed_reader.keys_at(&VPath::root()).unwrap().len(), 2);
        assert!(boxed_reader.exists_at(&vp("n")).unwrap());

        let arced_reader: Arc<dyn Reader> = Arc::new(built());
        assert_eq!(arced_reader.scalar_at(&vp("n")).unwrap(), Some(Scalar::I64(5)));
        assert_eq!(arced_reader.kind_at(&vp("n")).unwrap(), Some(NodeKind::Leaf));
        assert_eq!(arced_reader.len_at(&vp("xs")).unwrap(), 1);
        assert_eq!(arced_reader.keys_at(&VPath::root()).unwrap().len(), 2);
        assert!(arced_reader.exists_at(&vp("n")).unwrap());
    }
}
