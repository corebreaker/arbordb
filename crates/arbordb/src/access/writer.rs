//! Read/write access to one value's tree, addressed by an intra-value [`VPath`].

use super::Reader;
use crate::{data::Scalar, error::AdbResult, node::NodeKind, path::VPath};
use std::sync::Arc;

/// Read/write access to the node tree of a single value being built or edited.
pub trait Writer: Reader {
    /// Sets `scalar` at `at`, creating missing containers along the way.
    fn put_scalar(&self, at: &VPath, scalar: Scalar) -> AdbResult<()>;

    /// Ensures an (initially empty) container exists at `at` — a list when `list`,
    /// otherwise an object — so empty containers still materialize.
    fn ensure_container(&self, at: &VPath, list: bool) -> AdbResult<()>;

    /// Removes the subtree at `at`, returning whether anything was removed.
    fn remove(&self, at: &VPath) -> AdbResult<bool>;
}

impl Reader for Box<dyn Writer + '_> {
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>> {
        (**self).scalar_at(at)
    }

    fn kind_at(&self, at: &VPath) -> AdbResult<Option<NodeKind>> {
        (**self).kind_at(at)
    }

    fn len_at(&self, at: &VPath) -> AdbResult<usize> {
        (**self).len_at(at)
    }

    fn keys_at(&self, at: &VPath) -> AdbResult<Vec<String>> {
        (**self).keys_at(at)
    }

    fn exists_at(&self, at: &VPath) -> AdbResult<bool> {
        (**self).exists_at(at)
    }
}

impl Writer for Box<dyn Writer + '_> {
    fn put_scalar(&self, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        (**self).put_scalar(at, scalar)
    }

    fn ensure_container(&self, at: &VPath, list: bool) -> AdbResult<()> {
        (**self).ensure_container(at, list)
    }

    fn remove(&self, at: &VPath) -> AdbResult<bool> {
        (**self).remove(at)
    }
}

impl Reader for Arc<dyn Writer + '_> {
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>> {
        (**self).scalar_at(at)
    }

    fn kind_at(&self, at: &VPath) -> AdbResult<Option<NodeKind>> {
        (**self).kind_at(at)
    }

    fn len_at(&self, at: &VPath) -> AdbResult<usize> {
        (**self).len_at(at)
    }

    fn keys_at(&self, at: &VPath) -> AdbResult<Vec<String>> {
        (**self).keys_at(at)
    }

    fn exists_at(&self, at: &VPath) -> AdbResult<bool> {
        (**self).exists_at(at)
    }
}

impl Writer for Arc<dyn Writer + '_> {
    fn put_scalar(&self, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        (**self).put_scalar(at, scalar)
    }

    fn ensure_container(&self, at: &VPath, list: bool) -> AdbResult<()> {
        (**self).ensure_container(at, list)
    }

    fn remove(&self, at: &VPath) -> AdbResult<bool> {
        (**self).remove(at)
    }
}
