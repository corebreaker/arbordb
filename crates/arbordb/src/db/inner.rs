//! The database handle and the database-wide shared state behind it.

use crate::{
    error::{AdbError, AdbResult},
    cache::PathCache,
};

use redb::Database;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
        Mutex,
        RwLock,
    },
};

/// Database-wide shared state, held behind an [`Arc`] so an
/// [`ArborDb`](crate::ArborDb) is cheap to clone and share across threads.
pub(crate) struct DbInner {
    db:           Database,
    generation:   AtomicU64,
    version_lock: RwLock<()>,
    caches:       Mutex<HashMap<String, Arc<PathCache>>>,
}

impl DbInner {
    /// Bundles a fresh engine handle with its shared coordination state.
    pub(super) fn new(
        db: Database,
        generation: AtomicU64,
        version_lock: RwLock<()>,
        caches: Mutex<HashMap<String, Arc<PathCache>>>,
    ) -> Self {
        Self {
            db,
            generation,
            version_lock,
            caches,
        }
    }

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
