//! A handle to a single table, from which transactions are started.

use crate::{
    cache::PathCache,
    db::DbInner,
    error::{AdbError, AdbResult},
    txn::{ReadTxn, WriteTxn},
};

use redb::ReadableDatabase;
use std::sync::Arc;

/// A handle to one named table. Cheap to clone; reads are concurrent, writes are
/// serialized by the engine.
#[derive(Clone)]
pub struct Table {
    inner: Arc<DbInner>,
    name:  String,
    cache: Arc<PathCache>,
}

impl Table {
    pub(crate) fn new(inner: Arc<DbInner>, name: String, cache: Arc<PathCache>) -> Self {
        Self {
            inner,
            name,
            cache,
        }
    }

    /// The table's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Begins a read transaction — a consistent snapshot. The snapshot and its
    /// generation are captured atomically against concurrent commits.
    pub fn read(&self) -> AdbResult<ReadTxn> {
        let guard = self
            .inner
            .version_lock()
            .read()
            .map_err(|_| AdbError::CannotAccess(String::from("the version lock was poisoned")))?;

        let txn = self.inner.db().begin_read()?;
        let generation = self.inner.generation();
        drop(guard);

        Ok(ReadTxn::new(
            txn,
            self.name.clone(),
            Arc::clone(&self.cache),
            generation,
        ))
    }

    /// Begins a write transaction (serialized against other writers).
    pub fn write(&self) -> AdbResult<WriteTxn> {
        let txn = self.inner.db().begin_write()?;

        Ok(WriteTxn::new(txn, self.name.clone(), Arc::clone(&self.inner)))
    }
}
