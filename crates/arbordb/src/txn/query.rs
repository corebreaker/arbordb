//! The secondary-index query builder.
//!
//! [`ReadTxn::query`](super::ReadTxn::query) returns an [`IndexQuery`] that runs
//! an exact or **prefix** match against an index and recomposes each hit as a
//! `T: AData`. A query carries a **prefix** of column values (in the index's
//! column order — fewer than the index has columns is a prefix match, an empty
//! prefix matches every indexed entity, more is an [`IndexArity`] error), a
//! **direction** (index order by default, honoring each column's ASC/DESC, or
//! reversed), and an optional subtree **root** scope.
//!
//! [`IndexArity`]: crate::AdbError::IndexArity

use super::grab::{self, Grab};
use crate::{
    data::{AData, Scalar},
    error::AdbResult,
    path::APath,
};

/// A pending index query: an exact or prefix match against an index, optionally
/// reversed or subtree-scoped. Build it up, then [`run`](IndexQuery::run).
pub struct IndexQuery<'t> {
    /// The transaction the query runs against (a read or a write snapshot).
    txn:     &'t dyn Grab,
    /// The name of the index to scan.
    index:   String,
    /// The leading column values to match (empty matches every entity).
    prefix:  Vec<Scalar>,
    /// Whether to return hits in reverse index order.
    reverse: bool,
    /// The subtree scope; only entities at or under it are kept.
    root:    APath,
}

impl<'t> IndexQuery<'t> {
    pub(crate) fn new(txn: &'t dyn Grab, index: &str) -> Self {
        Self {
            txn,
            index: index.to_string(),
            prefix: Vec::new(),
            reverse: false,
            root: APath::root(),
        }
    }

    /// Matches entities whose leading columns equal `values` (in column order).
    /// Fewer values than the index has columns is a prefix match; the full set is
    /// an exact match; an empty prefix (the default) matches every indexed entity.
    pub fn prefixed(self, values: &[Scalar]) -> Self {
        Self {
            prefix: values.to_vec(),
            ..self
        }
    }

    /// Returns results in reverse index order instead of forward.
    pub fn reversed(self) -> Self {
        Self {
            reverse: true,
            ..self
        }
    }

    /// Keeps only entities at or under `root`.
    pub fn under(self, root: APath) -> Self {
        Self {
            root,
            ..self
        }
    }

    /// Runs the query, recomposing each matched entity as a `T`.
    pub fn run<T: AData>(self) -> AdbResult<Vec<T>> {
        grab::execute_query(self.txn, &self.index, &self.prefix, self.reverse, &self.root)
    }
}
