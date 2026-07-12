//! Conversions from the storage engine's error types into [`AdbError`].
//!
//! Kept in one place so the concrete engine (redb) never leaks into the public
//! API — every engine error surfaces as an opaque [`AdbError::Engine`].

use crate::error::AdbError;
use redb::{CommitError, DatabaseError, StorageError, TableError, TransactionError};

macro_rules! from_engine_error {
    ($($err:ty),+ $(,)?) => {
        $(
            impl From<$err> for AdbError {
                fn from(error: $err) -> Self {
                    AdbError::engine(error)
                }
            }
        )+
    };
}

from_engine_error!(DatabaseError, TransactionError, TableError, StorageError, CommitError);
