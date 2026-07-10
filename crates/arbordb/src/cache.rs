//! Per-table LRU caches that amortize path resolution and hot-node reads.
//!
//! Two caches, both keyed by the database **generation** so a snapshot never
//! reuses another version's resolution:
//!
//! - `paths`: an [`APath`] → `(generation, AKey)` map, so a resolved path skips re-walking the directory tree. The
//!   generation lives in the value (not the key), so a lookup borrows the caller's `&APath` and a stale-generation hit
//!   simply reads as a miss.
//! - `blobs`: a `(generation, AKey)` → entry-bytes map, so a hot directory or file is read from the engine once per
//!   generation and then navigated in place.
//!
//! Only [`ReadTxn`](crate::txn::ReadTxn) uses these; a writer sees its own
//! uncommitted state and must never cache.

use crate::{
    error::{AdbError, AdbResult},
    path::APath,
    AKey,
};

use lru::LruCache;
use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex, PoisonError},
};

/// Capacity of the path-resolution cache.
const PATH_CAPACITY: usize = 64 * 1024;

/// Capacity of the entry-blob cache.
const BLOB_CAPACITY: usize = 16 * 1024;

/// The path-resolution cache: an [`APath`] to its `(generation, key)`.
type PathEntries = LruCache<APath, (u64, AKey)>;

/// The entry-blob cache: a `(generation, key)` to the node's entry bytes.
type BlobEntries = LruCache<(u64, AKey), Arc<Vec<u8>>>;

/// A per-table pair of bounded LRU caches, shared across that table's read
/// transactions.
pub(crate) struct PathCache {
    paths: Mutex<PathEntries>,
    blobs: Mutex<BlobEntries>,
}

impl PathCache {
    /// A fresh, empty cache pair.
    pub(crate) fn new() -> Self {
        Self {
            paths: Mutex::new(LruCache::new(NonZeroUsize::new(PATH_CAPACITY).unwrap())),
            blobs: Mutex::new(LruCache::new(NonZeroUsize::new(BLOB_CAPACITY).unwrap())),
        }
    }

    /// The key `path` resolved to under `generation`, if cached and current.
    pub(crate) fn get_path(&self, generation: u64, path: &APath) -> AdbResult<Option<AKey>> {
        let mut paths = self.paths.lock().map_err(Self::poisoned)?;

        Ok(paths
            .get(path)
            .and_then(|(stored, akey)| (*stored == generation).then_some(*akey)))
    }

    /// Records that `path` resolves to `akey` under `generation`.
    pub(crate) fn put_path(&self, generation: u64, path: APath, akey: AKey) -> AdbResult<()> {
        self.paths.lock().map_err(Self::poisoned)?.put(path, (generation, akey));

        Ok(())
    }

    /// The cached entry blob for `akey` under `generation`, if present.
    pub(crate) fn get_blob(&self, generation: u64, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>> {
        let mut blobs = self.blobs.lock().map_err(Self::poisoned)?;

        Ok(blobs.get(&(generation, akey)).cloned())
    }

    /// Caches `blob` for `akey` under `generation`.
    pub(crate) fn put_blob(&self, generation: u64, akey: AKey, blob: Arc<Vec<u8>>) -> AdbResult<()> {
        self.blobs.lock().map_err(Self::poisoned)?.put((generation, akey), blob);

        Ok(())
    }

    /// Maps a poisoned cache lock to an [`AdbError`].
    fn poisoned<T>(_: PoisonError<T>) -> AdbError {
        AdbError::CannotAccess(String::from("a cache lock was poisoned"))
    }
}
