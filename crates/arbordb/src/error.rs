//! Error and result types for the public API.

use crate::path::{APath, VPath};
use std::error::Error;

/// Result type used throughout the public API.
pub type AdbResult<T> = Result<T, AdbError>;

/// Errors returned by ArborDb.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AdbError {
    /// An error originating from the underlying storage engine.
    ///
    /// The concrete engine type is intentionally kept private so that the
    /// storage backend remains an implementation detail.
    #[error("storage engine error: {0}")]
    Engine(#[source] Box<dyn Error + Send + Sync + 'static>),

    /// A path string could not be parsed into an [`APath`] or [`VPath`].
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// A table name was rejected (empty or reserved).
    #[error("invalid table name: {0}")]
    InvalidTableName(String),

    /// No value is stored at the requested access path.
    #[error("value not found: {0}")]
    ValueNotFound(APath),

    /// No node exists at the requested path inside a value.
    #[error("path not found: {0}")]
    PathNotFound(VPath),

    /// The node at a path was not of the kind the operation required.
    #[error("unexpected node at '{path}': expected {expected}, found {found}")]
    UnexpectedNode {
        /// The path that was being accessed.
        path:     VPath,
        /// The node kind the operation required.
        expected: &'static str,
        /// The node kind actually stored.
        found:    &'static str,
    },

    /// A byte slice or string could not be converted into a valid [`AKey`](crate::AKey).
    #[error("invalid key: {0}")]
    BadKey(String),

    /// A scalar could not be read as the requested Rust type.
    #[error("type mismatch: expected {expected}, found {found}")]
    TypeMismatch {
        /// The Rust type the caller asked for.
        expected: &'static str,
        /// The scalar variant actually stored.
        found:    &'static str,
    },

    /// A list index referenced a position past the end of the list.
    #[error("list index out of range at '{path}': {index} (length {len})")]
    IndexOutOfRange {
        /// The list-element path that was rejected.
        path:  VPath,
        /// The requested index.
        index: u64,
        /// The current list length.
        len:   u64,
    },

    /// Stored bytes could not be decoded (corruption or format skew).
    #[error("corrupted data: {0}")]
    Corrupt(String),

    /// A unique index constraint was violated.
    #[error("unique index '{index}' violated")]
    UniqueViolation {
        /// The name of the violated index.
        index: String,
    },

    /// A query referenced an index that is not registered on the table.
    #[error("index not found: {index}")]
    IndexNotFound {
        /// The index name that was requested.
        index: String,
    },

    /// An index query supplied the wrong number of column values.
    #[error("index '{index}' takes {expected} column value(s), got {got}")]
    IndexArity {
        /// The index being queried.
        index:    String,
        /// The number of columns the index has.
        expected: usize,
        /// The number of values supplied.
        got:      usize,
    },

    /// The on-disk format/schema does not match this build.
    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    /// A data access cannot be fulfilled.
    #[error("impossible access: {0}")]
    CannotAccess(String),

    /// A `try_from` conversion (`#[arbor(try_from = ...)]`) rejected a loaded value.
    #[error("conversion failed: {0}")]
    Conversion(String),
}

impl AdbError {
    /// Wraps a storage-engine error, keeping its source chain but hiding the
    /// concrete type from the public API.
    pub(crate) fn engine<E>(error: E) -> Self
    where
        E: Error + Send + Sync + 'static, {
        AdbError::Engine(Box::new(error))
    }
}
