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

    /// No vnode exists at the requested path inside a value.
    #[error("path not found: {0}")]
    PathNotFound(VPath),

    /// The vnode at a path was not of the kind the operation required.
    #[error("unexpected vnode at '{path}': expected {expected}, found {found}")]
    UnexpectedNode {
        /// The path that was being accessed.
        path:     VPath,
        /// The vnode kind the operation required.
        expected: &'static str,
        /// The vnode kind actually stored.
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

    /// The database file is access-controlled (it requires the `permissions`
    /// feature), but this build was compiled without that feature.
    #[error("this database is protected and requires the permissions feature to open")]
    DatabaseProtected,

    /// The database file requires an optional on-disk feature this build lacks.
    #[error("this database requires the '{feature}' feature, which this build lacks")]
    FeatureRequired {
        /// The cargo feature name the file needs the reader to have been built with.
        feature: String,
    },

    /// Authentication failed: the user is unknown or the password is wrong. The
    /// two cases are deliberately indistinguishable.
    #[error("authentication failed")]
    AuthenticationFailed,

    /// The authenticated principal lacks a right required for the operation.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// An authenticated operation was attempted on a database that has no
    /// permission system (it was created without the `permissions` feature).
    #[error("this database has no permission system")]
    NoPermissions,

    /// A reserved (`$`-prefixed) table's contents failed their integrity check —
    /// evidence of tampering or corruption outside the library.
    #[error("integrity check failed: {0}")]
    Tampered(String),

    /// An operation targeted a frozen principal (such as moving the `guest` user
    /// into a group), which is immutable by design.
    #[error("'{0}' is frozen and cannot be modified")]
    Frozen(String),

    /// A Serde `Serialize` or `Deserialize` failed while mapping to or from ArborDb's
    /// value codec (for `store_serde_value` / `load_serde_value`).
    #[cfg(feature = "serde")]
    #[error("serde error: {0}")]
    Serde(String),
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

#[cfg(feature = "serde")]
impl serde::ser::Error for AdbError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        AdbError::Serde(msg.to_string())
    }
}

#[cfg(feature = "serde")]
impl serde::de::Error for AdbError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        AdbError::Serde(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_wraps_a_source_error_opaquely() {
        let err = AdbError::engine(std::io::Error::other("boom"));

        assert!(matches!(err, AdbError::Engine(_)));
        assert!(err.to_string().contains("storage engine error"));
    }
}
