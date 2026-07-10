//! The database handle and the database-wide shared state behind it.

use super::DbInner;
use crate::{
    constants::{INDEX_TABLE_NAME, METADATA_TABLE_NAME},
    engine,
    error::{AdbError, AdbResult},
    table::Table,
};

use redb::{backends::InMemoryBackend, Database, ReadableDatabase, TableHandle};
use std::{
    collections::HashMap,
    path::Path,
    sync::{atomic::AtomicU64, Arc, Mutex, RwLock},
};

/// An ArborDb database: one file (or an in-memory store) holding any number of
/// named [`Table`]s.
#[derive(Clone)]
pub struct ArborDb {
    inner: Arc<DbInner>,
}

impl ArborDb {
    /// Creates a new database file at `path` (truncating any existing one).
    pub fn create(path: impl AsRef<Path>) -> AdbResult<Self> {
        Self::wrap(Database::create(path)?)
    }

    /// Opens an existing database file at `path`.
    pub fn open(path: impl AsRef<Path>) -> AdbResult<Self> {
        Self::wrap(Database::open(path)?)
    }

    /// Creates a database kept entirely in memory (useful for tests).
    pub fn create_in_memory() -> AdbResult<Self> {
        Self::wrap(Database::builder().create_with_backend(InMemoryBackend::new())?)
    }

    /// Bootstraps the metadata table then wraps the engine handle.
    fn wrap(db: Database) -> AdbResult<Self> {
        engine::bootstrap_metadata(&db)?;

        Ok(Self {
            inner: Arc::new(DbInner::new(
                db,
                AtomicU64::new(0),
                RwLock::new(()),
                Mutex::new(HashMap::new()),
            )),
        })
    }

    /// Opens a handle to the table named `name`, from which transactions start.
    /// The table is created lazily on its first write. The reserved `$metadata`
    /// name and the empty name are rejected.
    pub fn open_table(&self, name: &str) -> AdbResult<Table> {
        if name.is_empty() {
            return Err(AdbError::InvalidTableName(String::from("a table name cannot be empty")));
        }

        if name == METADATA_TABLE_NAME || name == INDEX_TABLE_NAME {
            return Err(AdbError::InvalidTableName(format!("'{name}' is reserved")));
        }

        let cache = self.inner.cache(name)?;

        Ok(Table::new(Arc::clone(&self.inner), name.to_string(), cache))
    }

    /// The names of every user table, in sorted order.
    ///
    /// The reserved `$metadata` and `$index` tables are omitted. A table is
    /// created lazily on its first write, so one that was
    /// [`open_table`](Self::open_table)ed but never written to does not appear yet.
    pub fn list_tables(&self) -> AdbResult<Vec<String>> {
        let txn = self.inner.db().begin_read()?;

        // Collect owned names before the transaction is dropped; the reserved
        // tables are engine bookkeeping, not user tables.
        let mut names: Vec<String> = txn
            .list_tables()?
            .map(|handle| handle.name().to_string())
            .filter(|name| name != METADATA_TABLE_NAME && name != INDEX_TABLE_NAME)
            .collect();

        names.sort();

        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{data::Scalar, Value};

    #[test]
    fn list_tables_reports_written_user_tables_only() {
        let db = ArborDb::create_in_memory().unwrap();

        // A fresh database exposes no user tables — the reserved `$metadata` table
        // is present but filtered out.
        assert!(db.list_tables().unwrap().is_empty());

        // A table opened but never written to is not materialized yet.
        let _pending = db.open_table("pending").unwrap();
        assert!(db.list_tables().unwrap().is_empty());

        // Two written tables show up in sorted order, without the reserved tables.
        for name in ["zeta", "alpha"] {
            let table = db.open_table(name).unwrap();
            let w = table.write().unwrap();
            w.store_value("k", &Value::Leaf(Scalar::I64(1))).unwrap();
            w.commit().unwrap();
        }

        assert_eq!(db.list_tables().unwrap(), vec!["alpha", "zeta"]);
    }
}
