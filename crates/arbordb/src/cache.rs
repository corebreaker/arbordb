//! Per-table LRU caches that amortize path resolution and hot-vnode reads.
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

/// Capacity of the inode-bytes cache (protected databases only).
#[cfg(feature = "permissions")]
const INODE_CAPACITY: usize = 16 * 1024;

/// The path-resolution cache: an [`APath`] to its `(generation, key)`.
type PathEntries = LruCache<APath, (u64, AKey)>;

/// The entry-blob cache: a `(generation, key)` to the vnode's entry bytes.
type BlobEntries = LruCache<(u64, AKey), Arc<Vec<u8>>>;

/// The inode-bytes cache: a `(generation, key)` to the vnode's raw `$inodes` blob.
#[cfg(feature = "permissions")]
type InodeEntries = LruCache<(u64, AKey), Arc<Vec<u8>>>;

/// The MAC-verified set: the `(generation, key)`s whose blob an authenticated reader
/// has already checked this generation (the unit value is just set membership).
#[cfg(feature = "permissions")]
type VerifiedEntries = LruCache<(u64, AKey), ()>;

/// A per-table pair of bounded LRU caches, shared across that table's read
/// transactions.
pub(crate) struct PathCache {
    /// The path-resolution cache (`APath` → `(generation, key)`).
    paths:  Mutex<PathEntries>,
    /// The entry-blob cache (`(generation, key)` → entry bytes).
    blobs:  Mutex<BlobEntries>,
    /// The inode-bytes cache (`(generation, key)` → raw `$inodes` blob). Holds only
    /// the undecoded, principal-independent metadata bytes, so it is sound to share
    /// across principals — the ACL check and integrity verify still run per read on
    /// the reading principal (a protected database only).
    #[cfg(feature = "permissions")]
    inodes: Mutex<InodeEntries>,

    /// The set of `(generation, key)`s an authenticated reader has already
    /// MAC-verified this generation. Within a generation the snapshot is immutable
    /// (redb MVCC), so re-verifying the same blob is redundant; the keyed MAC is keyed
    /// by the database-global integrity key, so the outcome is principal-independent
    /// and this may be shared. Empty after a reopen, so at-rest tampering is still
    /// caught on first read. Populated only on the authenticated-user (keyed-MAC)
    /// path — never for a guest, whose signature check depends on its (possibly
    /// pinned) public key.
    #[cfg(feature = "permissions")]
    verified_macs: Mutex<VerifiedEntries>,
}

impl PathCache {
    /// A fresh, empty cache pair.
    pub(crate) fn new() -> Self {
        Self {
            paths:                                         Mutex::new(LruCache::new(
                NonZeroUsize::new(PATH_CAPACITY).unwrap(),
            )),
            blobs:                                         Mutex::new(LruCache::new(
                NonZeroUsize::new(BLOB_CAPACITY).unwrap(),
            )),
            #[cfg(feature = "permissions")]
            inodes:                                        Mutex::new(LruCache::new(
                NonZeroUsize::new(INODE_CAPACITY).unwrap(),
            )),
            #[cfg(feature = "permissions")]
            verified_macs:                                 Mutex::new(LruCache::new(
                NonZeroUsize::new(INODE_CAPACITY).unwrap(),
            )),
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

    /// The cached raw inode blob for `akey` under `generation`, if present.
    #[cfg(feature = "permissions")]
    pub(crate) fn get_inode(&self, generation: u64, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>> {
        let mut inodes = self.inodes.lock().map_err(Self::poisoned)?;

        Ok(inodes.get(&(generation, akey)).cloned())
    }

    /// Caches the raw inode blob `bytes` for `akey` under `generation`.
    #[cfg(feature = "permissions")]
    pub(crate) fn put_inode(&self, generation: u64, akey: AKey, bytes: Arc<Vec<u8>>) -> AdbResult<()> {
        self.inodes
            .lock()
            .map_err(Self::poisoned)?
            .put((generation, akey), bytes);

        Ok(())
    }

    /// Whether `akey`'s blob was already MAC-verified under `generation`.
    #[cfg(feature = "permissions")]
    pub(crate) fn get_mac_verified(&self, generation: u64, akey: AKey) -> AdbResult<bool> {
        Ok(self
            .verified_macs
            .lock()
            .map_err(Self::poisoned)?
            .get(&(generation, akey))
            .is_some())
    }

    /// Records that `akey`'s blob has been MAC-verified under `generation`.
    #[cfg(feature = "permissions")]
    pub(crate) fn put_mac_verified(&self, generation: u64, akey: AKey) -> AdbResult<()> {
        self.verified_macs
            .lock()
            .map_err(Self::poisoned)?
            .put((generation, akey), ());

        Ok(())
    }

    /// Maps a poisoned cache lock to an [`AdbError`].
    fn poisoned<T>(_: PoisonError<T>) -> AdbError {
        AdbError::CannotAccess(String::from("a cache lock was poisoned"))
    }
}
