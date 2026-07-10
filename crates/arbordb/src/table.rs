//! A handle to a single table, from which transactions are started.

use crate::{
    db::DbInner,
    error::AdbResult,
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
}

impl Table {
    pub(crate) fn new(inner: Arc<DbInner>, name: String) -> Self {
        Self {
            inner,
            name,
        }
    }

    /// The table's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Begins a read transaction — a consistent snapshot.
    pub fn read(&self) -> AdbResult<ReadTxn> {
        let txn = self.inner.db().begin_read()?;

        Ok(ReadTxn::new(txn, self.name.clone()))
    }

    /// Begins a write transaction (serialized against other writers).
    pub fn write(&self) -> AdbResult<WriteTxn> {
        let txn = self.inner.db().begin_write()?;

        Ok(WriteTxn::new(txn, self.name.clone()))
    }
}
