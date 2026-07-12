//! A per-write-transaction write-back cache of dirty a-node entries — directory
//! child-maps and edited file values alike — flushed (and, under `permissions`,
//! sealed) once at commit.
//!
//! **Directories.** Linking a child into a directory rewrites that directory's whole
//! blob, so a plain loop of `store` under one parent rewrites (and re-encodes) it once
//! per child — O(N²). With buffering on, a directory mutated in the transaction keeps
//! its child-map here, decoded and mutated **in place**; its blob is encoded and
//! written to the engine just once, at commit — O(N).
//!
//! **Files.** Editing a file's value re-encodes and rewrites (and, under `permissions`,
//! re-signs with an ~10µs Ed25519 signature) its whole blob, so a burst of edits to
//! one value in a transaction would pay that once per edit. With buffering on, an
//! edited file's entry bytes are kept here, and written (and signed) just once, at
//! commit. Only *edits* buffer — a fresh `store` writes through, so a new file's ACL
//! and seal land at once (nothing here needs an eager ACL).
//!
//! A buffered entry is authoritative and trusted: every read within the transaction
//! consults it before the engine, and it needs no integrity re-check (it is the
//! transaction's own state; it is sealed only at the commit-time flush).
//!
//! Buffering is enabled only for an **index-free** database, so index maintenance —
//! which reads the engine directly — always sees the current structure.
//!
//! Lock-free `dirty` flags let a read skip the mutex entirely until something is
//! actually buffered, so a transaction that neither grows a directory nor re-edits a
//! file (a fresh bulk load, a single overwrite) pays nothing for the cache.

use crate::AKey;
use smol_str::SmolStr;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};

/// The children of a single directory, buffered for in-place mutation. A short child
/// name (the common case) is stored inline by [`SmolStr`], so seeding and mutating a
/// buffered directory allocates nothing per name.
type Children = BTreeMap<SmolStr, AKey>;

/// The dirty a-node entries buffered so far in one write transaction, plus the flags
/// that gate access to them. A [`Mutex`] (not a `RefCell`) keeps the enclosing write
/// transaction — and a mutable accessor borrowing it — `Sync`; it is only ever locked
/// single-threaded.
pub(in crate::txn) struct WriteBuffer {
    /// Whether writes buffer here at all. Off for an indexed database (its index
    /// maintenance reads the engine directly, so writes must land there).
    buffering:   AtomicBool,
    /// Whether any directory is currently buffered — the cheap gate a directory read
    /// checks before the mutex.
    dirs_dirty:  AtomicBool,
    /// The buffered directory child-maps, keyed by a-node.
    dirs:        Mutex<HashMap<AKey, Children>>,
    /// Whether any file is currently buffered — the cheap gate a file read checks
    /// before the mutex.
    files_dirty: AtomicBool,
    /// The buffered file entry blobs (tag + value blob), keyed by a-node.
    files:       Mutex<HashMap<AKey, Vec<u8>>>,
    /// A monotonic count of structural changes to the directory tree — every
    /// `name → child` relink (a child link or unlink) bumps it. A mutable cursor
    /// captures it when opened and its resolved-key hint is trusted only while the
    /// count is unchanged, so an interleaved `mv`/`rm`/`store` that could repoint the
    /// cursor's path invalidates the hint rather than silently patching a relinked
    /// a-node (see `resolve_target`).
    structure:   AtomicU64,
}

impl WriteBuffer {
    /// A fresh buffer. With `buffering` off the writers bypass it (writes go direct).
    pub(in crate::txn) fn new(buffering: bool) -> Self {
        Self {
            buffering:   AtomicBool::new(buffering),
            dirs_dirty:  AtomicBool::new(false),
            dirs:        Mutex::new(HashMap::new()),
            files_dirty: AtomicBool::new(false),
            files:       Mutex::new(HashMap::new()),
            structure:   AtomicU64::new(0),
        }
    }

    /// Whether writes should buffer here rather than hit the engine.
    pub(in crate::txn) fn active(&self) -> bool {
        self.buffering.load(Ordering::Relaxed)
    }

    // -- Directories ---------------------------------------------------------

    /// Whether any directory is buffered — the cheap gate a directory read checks
    /// before the mutex.
    pub(in crate::txn) fn dirty(&self) -> bool {
        self.dirs_dirty.load(Ordering::Relaxed)
    }

    /// Locks the directory buffer (recovering it through a — single-threaded, so
    /// unreachable — poison rather than wedging the write) and reads it through `f`.
    /// Callers gate this on [`dirty`](Self::dirty), so it is not taken until a
    /// directory is buffered.
    pub(in crate::txn) fn read<R>(&self, f: impl FnOnce(&HashMap<AKey, Children>) -> R) -> R {
        f(&self.dirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Locks the directory buffer, mutates it through `f`, and marks it dirty.
    pub(in crate::txn) fn write<R>(&self, f: impl FnOnce(&mut HashMap<AKey, Children>) -> R) -> R {
        let result = f(&mut self.dirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        self.dirs_dirty.store(true, Ordering::Relaxed);

        result
    }

    // -- Files ---------------------------------------------------------------

    /// Whether any file is buffered — the cheap gate a file read checks before the
    /// mutex.
    pub(in crate::txn) fn files_dirty(&self) -> bool {
        self.files_dirty.load(Ordering::Relaxed)
    }

    /// The buffered entry bytes for file `akey`, if it has been edited in this
    /// transaction. Clones them out so the lock is not held across the caller's use;
    /// a buffered file is being actively edited, so the copy is cheap relative to the
    /// re-encode (and, under `permissions`, the signature) it defers.
    pub(in crate::txn) fn file_get(&self, akey: AKey) -> Option<Vec<u8>> {
        if !self.files_dirty() {
            return None;
        }

        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&akey)
            .cloned()
    }

    /// Whether file `akey` is currently buffered — the cheap presence check the
    /// (permissions-only) verify skip needs, without cloning the bytes.
    #[cfg(feature = "permissions")]
    pub(in crate::txn) fn file_contains(&self, akey: AKey) -> bool {
        self.files_dirty()
            && self
                .files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(&akey)
    }

    /// Buffers `entry` as file `akey`'s current bytes, to be written (and sealed) once
    /// at commit.
    pub(in crate::txn) fn file_put(&self, akey: AKey, entry: Vec<u8>) {
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(akey, entry);

        self.files_dirty.store(true, Ordering::Relaxed);
    }

    /// Drops file `akey` from the buffer — a direct engine write of it supersedes any
    /// buffered edit. Lock-free, and a no-op, when no file is buffered (the common
    /// case: a fresh bulk load never buffers a file), so a write-through pays nothing.
    pub(in crate::txn) fn file_forget(&self, akey: AKey) {
        if self.files_dirty() {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&akey);
        }
    }

    // -- Structural-change counter -------------------------------------------

    /// The current structural-change count. A mutable cursor captures it at
    /// `fetch_mut`, and its resolved-key hint is trusted only while the count is
    /// unchanged (see `resolve_target`).
    pub(in crate::txn) fn structure_epoch(&self) -> u64 {
        self.structure.load(Ordering::Relaxed)
    }

    /// Records a structural change — a `name → child` relink — so any cursor hint
    /// captured before it is no longer trusted.
    pub(in crate::txn) fn bump_structure(&self) {
        self.structure.fetch_add(1, Ordering::Relaxed);
    }

    // -- Shared --------------------------------------------------------------

    /// Drops `akey` from the buffer (its a-node was removed or replaced), so it is not
    /// flushed — lock-free, and a no-op, when nothing of its kind is buffered.
    pub(in crate::txn) fn forget(&self, akey: AKey) {
        if self.dirty() {
            self.dirs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&akey);
        }

        if self.files_dirty() {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&akey);
        }
    }

    /// Takes every buffered directory and file out, clearing the dirty flags — the
    /// commit-time flush.
    pub(in crate::txn) fn take(&self) -> (HashMap<AKey, Children>, HashMap<AKey, Vec<u8>>) {
        self.dirs_dirty.store(false, Ordering::Relaxed);
        self.files_dirty.store(false, Ordering::Relaxed);

        let dirs = std::mem::take(&mut *self.dirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        let files = std::mem::take(&mut *self.files.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));

        (dirs, files)
    }

    /// Takes every buffered entry **and turns buffering off** for the rest of the
    /// transaction. Used the moment a mutation needs the engine to reflect the current
    /// structure — index maintenance reads the engine directly — so no later write
    /// hides an entry in the buffer where that maintenance would miss it.
    pub(in crate::txn) fn take_and_disable(&self) -> (HashMap<AKey, Children>, HashMap<AKey, Vec<u8>>) {
        self.buffering.store(false, Ordering::Relaxed);

        self.take()
    }
}
