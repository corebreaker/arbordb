//! The database handle and the shared state behind it.

use crate::{
    constants::METADATA_TABLE_NAME,
    engine,
    error::{AdbError, AdbResult},
    table::Table,
};

use redb::{backends::InMemoryBackend, Database};
use std::{path::Path, sync::Arc};

/// Database-wide shared state, held behind an [`Arc`] so an [`ArborDb`] is cheap
/// to clone and share across threads.
pub(crate) struct DbInner {
    db: Database,
}

impl DbInner {
    /// The underlying engine handle.
    pub(crate) fn db(&self) -> &Database {
        &self.db
    }
}

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
            inner: Arc::new(DbInner {
                db,
            }),
        })
    }

    /// Opens a handle to the table named `name`, from which transactions start.
    /// The table is created lazily on its first write. The reserved `$metadata`
    /// name and the empty name are rejected.
    pub fn open_table(&self, name: &str) -> AdbResult<Table> {
        if name.is_empty() {
            return Err(AdbError::InvalidTableName(String::from("a table name cannot be empty")));
        }

        if name == METADATA_TABLE_NAME {
            return Err(AdbError::InvalidTableName(format!("'{name}' is reserved")));
        }

        Ok(Table::new(Arc::clone(&self.inner), name.to_string()))
    }
}
