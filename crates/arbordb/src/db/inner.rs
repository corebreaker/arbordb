//! The database handle and the database-wide shared state behind it.

use crate::{
    error::{AdbError, AdbResult},
    cache::PathCache,
};

use redb::Database;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
        Mutex,
        RwLock,
    },
};

/// Database-wide shared state, held behind an [`Arc`] so an
/// [`ArborDb`](crate::ArborDb) is cheap to clone and share across threads.
pub(crate) struct DbInner {
    /// The underlying storage-engine handle.
    db:           Database,
    /// The database-wide version counter, bumped on every committed write.
    generation:   AtomicU64,
    /// Serializes a read's `(snapshot, generation)` capture against a commit's
    /// `(commit, generation bump)` so a snapshot never borrows another version.
    version_lock: RwLock<()>,
    /// Per-table caches, created lazily on first use.
    caches:       Mutex<HashMap<String, Arc<PathCache>>>,
    /// Whether any secondary index exists anywhere in the database. Primed at open
    /// and set on index creation; lets a mutation on an index-free database skip the
    /// registry read entirely. Only ever set (never cleared), so it is a conservative
    /// hint — a stale `true` merely takes the slower, still-correct path.
    has_indexes:  AtomicBool,

    /// Buffered a-node access times awaiting a flush to `$inodes`: reads record here
    /// (cheap, in memory) and a committed write or an explicit flush persists them.
    /// Keyed by `(table, a-node)`, valued by the latest access time (epoch millis).
    #[cfg(feature = "entry-timestamps")]
    access_log: Mutex<HashMap<(String, crate::AKey), i64>>,
}

impl DbInner {
    /// Bundles a fresh engine handle with its shared coordination state.
    pub(super) fn new(
        db: Database,
        generation: AtomicU64,
        version_lock: RwLock<()>,
        caches: Mutex<HashMap<String, Arc<PathCache>>>,
        has_indexes: AtomicBool,
    ) -> Self {
        Self {
            db,
            generation,
            version_lock,
            caches,
            has_indexes,
            #[cfg(feature = "entry-timestamps")]
            access_log: Mutex::new(HashMap::new()),
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

    /// Whether any secondary index might exist — a conservative hint the write path
    /// checks before reading the registry (a `false` is exact: no index exists).
    pub(crate) fn has_any_index(&self) -> bool {
        self.has_indexes.load(Ordering::Acquire)
    }

    /// Records that a secondary index now exists, so later mutations take the
    /// index-maintaining path.
    pub(crate) fn mark_has_index(&self) {
        self.has_indexes.store(true, Ordering::Release);
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

    /// Buffers access times for `table`'s nodes, keeping the latest time per a-node.
    /// Called when a read transaction ends; best-effort (a poisoned lock drops the
    /// batch rather than propagating, since access times are advisory).
    #[cfg(feature = "entry-timestamps")]
    pub(crate) fn deposit_access(&self, table: &str, entries: Vec<(crate::AKey, i64)>) {
        if entries.is_empty() {
            return;
        }

        let Ok(mut log) = self.access_log.lock() else {
            return;
        };

        for (akey, when) in entries {
            log.entry((table.to_string(), akey))
                .and_modify(|current| *current = (*current).max(when))
                .or_insert(when);
        }
    }

    /// Drains every buffered access time, for a flush into `$inodes`.
    #[cfg(feature = "entry-timestamps")]
    pub(crate) fn drain_access_log(&self) -> Vec<((String, crate::AKey), i64)> {
        match self.access_log.lock() {
            Ok(mut log) => log.drain().collect(),
            Err(_) => Vec::new(),
        }
    }
}
