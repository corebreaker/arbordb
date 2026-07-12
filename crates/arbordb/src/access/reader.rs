//! Read access to one value's tree, addressed by an intra-value [`VPath`].

use crate::{data::Scalar, error::AdbResult, vnode::NodeKind, path::VPath};
use std::sync::Arc;

/// Read access to the vnode tree of a single stored value.
///
/// Every method addresses a vnode by a [`VPath`] relative to the value's root.
/// Implemented by the codec-backed cursor over a stored blob and by the in-memory
/// builder; accessors hold one behind an `Arc<dyn Reader + 't>`.
pub trait Reader {
    /// The scalar at `at`, or `None` if nothing is there or it is not a leaf.
    fn scalar_at(&self, at: &VPath) -> AdbResult<Option<Scalar>>;

    /// The vnode kind at `at`, or `None` if nothing is there.
    fn kind_at(&self, at: &VPath) -> AdbResult<Option<NodeKind>>;

    /// The length of the list at `at`; errors if `at` is not a list.
    fn len_at(&self, at: &VPath) -> AdbResult<usize>;

    /// The field names (in order) of the object at `at`; errors if `at` is not an
    /// object.
    fn keys_at(&self, at: &VPath) -> AdbResult<Vec<String>>;

    /// Whether any vnode exists at `at`.
    fn exists_at(&self, at: &VPath) -> AdbResult<bool>;
}

impl Reader for Box<dyn Reader + '_> {
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

impl Reader for Arc<dyn Reader + '_> {
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
