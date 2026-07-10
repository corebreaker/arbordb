//! The database handle and the database-wide shared state behind it.

use crate::{
    cache::PathCache,
    constants::{INDEX_TABLE_NAME, METADATA_TABLE_NAME},
    engine,
    error::{AdbError, AdbResult},
    table::Table,
};

use redb::{backends::InMemoryBackend, Database};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
        Mutex,
        RwLock,
    },
};

/// Database-wide shared state, held behind an [`Arc`] so an [`ArborDb`] is cheap
/// to clone and share across threads.
pub(crate) struct DbInner {
    db:           Database,
    generation:   AtomicU64,
    version_lock: RwLock<()>,
    caches:       Mutex<HashMap<String, Arc<PathCache>>>,
}

impl DbInner {
    /// The underlying engine handle.
    pub(crate) fn db(&self) -> &Database {
        &self.db
    }

    /// The current generation (bumped on every committed write).
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Advances the generation after a commit, invalidating older cache entries.
    pub(crate) fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// The lock serializing a read's `(snapshot, generation)` capture against a
    /// commit's `(commit, generation bump)`.
    pub(crate) fn version_lock(&self) -> &RwLock<()> {
        &self.version_lock
    }

    /// The shared cache for table `name`, created on first use.
    pub(crate) fn cache(&self, name: &str) -> AdbResult<Arc<PathCache>> {
        let mut caches = self
            .caches
            .lock()
            .map_err(|_| AdbError::CannotAccess(String::from("the cache registry lock was poisoned")))?;

        let cache = caches
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(PathCache::new()));

        Ok(Arc::clone(cache))
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
                generation: AtomicU64::new(0),
                version_lock: RwLock::new(()),
                caches: Mutex::new(HashMap::new()),
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

        if name == METADATA_TABLE_NAME || name == INDEX_TABLE_NAME {
            return Err(AdbError::InvalidTableName(format!("'{name}' is reserved")));
        }

        let cache = self.inner.cache(name)?;

        Ok(Table::new(Arc::clone(&self.inner), name.to_string(), cache))
    }
}
