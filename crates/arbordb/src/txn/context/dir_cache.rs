//! A per-write-transaction write-back cache of directory child-maps.
//!
//! Linking a child into a directory rewrites that directory's whole blob, so a plain
//! loop of `store` under one parent rewrites (and re-encodes) it once per child —
//! O(N²). With buffering on, a directory mutated in the transaction keeps its
//! child-map here, decoded and mutated **in place**; its blob is encoded and written
//! to the engine just once, at commit — O(N). A cached directory is authoritative
//! and trusted: every directory read within the transaction consults it before the
//! engine, and it needs no integrity re-check (it is the transaction's own state).
//!
//! Buffering is enabled only for an **index-free** database, so index maintenance —
//! which reads the engine directly — always sees the current directory structure.
//! The whole cache is compiled out under `permissions`, where directory reads verify
//! integrity tags and ACL checks must observe each write immediately.
//!
//! A lock-free `dirty` flag lets a read skip the mutex entirely until some directory
//! is actually buffered, so a transaction that never grows a directory (an overwrite,
//! a scalar edit) pays nothing for the cache.

use crate::AKey;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

/// The children of a single directory, buffered for in-place mutation.
type Children = BTreeMap<String, AKey>;

/// The directories mutated so far in one write transaction, plus the flags that gate
/// access to them. A [`Mutex`] (not a `RefCell`) keeps the enclosing write
/// transaction — and a mutable accessor borrowing it — `Sync`; it is only ever
/// locked single-threaded.
pub(in crate::txn) struct DirBuffer {
    /// Whether directory writes buffer here at all. Off for an indexed database (its
    /// index maintenance reads the engine directly, so writes must land there).
    buffering: AtomicBool,
    /// Whether any directory is currently buffered. A read checks this first and, when
    /// it is clear, skips the mutex — so a write transaction that never grows a
    /// directory pays nothing.
    dirty:     AtomicBool,
    /// The buffered directories, keyed by vnode.
    dirs:      Mutex<HashMap<AKey, Children>>,
}

impl DirBuffer {
    /// A fresh buffer. With `buffering` off the writers bypass it (writes go direct).
    pub(in crate::txn) fn new(buffering: bool) -> Self {
        Self {
            buffering: AtomicBool::new(buffering),
            dirty:     AtomicBool::new(false),
            dirs:      Mutex::new(HashMap::new()),
        }
    }

    /// Whether directory writes should buffer here rather than hit the engine.
    pub(in crate::txn) fn active(&self) -> bool {
        self.buffering.load(Ordering::Relaxed)
    }

    /// Whether any directory is buffered — the cheap gate a read checks before the
    /// mutex.
    pub(in crate::txn) fn dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Locks the buffer (recovering it through a — single-threaded, so unreachable —
    /// poison rather than wedging the write) and reads it through `f`. Callers gate
    /// this on [`dirty`](Self::dirty), so it is not taken until something is buffered.
    pub(in crate::txn) fn read<R>(&self, f: impl FnOnce(&HashMap<AKey, Children>) -> R) -> R {
        f(&self.dirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Locks the buffer, mutates it through `f`, and marks it dirty.
    pub(in crate::txn) fn write<R>(&self, f: impl FnOnce(&mut HashMap<AKey, Children>) -> R) -> R {
        let result = f(&mut self.dirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        self.dirty.store(true, Ordering::Relaxed);

        result
    }

    /// Drops `akey` from the buffer (its vnode was removed), so it is not flushed —
    /// a no-op, and lock-free, when nothing is buffered.
    pub(in crate::txn) fn forget(&self, akey: AKey) {
        if self.dirty() {
            self.dirs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&akey);
        }
    }

    /// Takes every buffered directory out, clearing the dirty flag — the commit-time
    /// flush.
    pub(in crate::txn) fn take(&self) -> HashMap<AKey, Children> {
        self.dirty.store(false, Ordering::Relaxed);

        std::mem::take(&mut self.dirs.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Takes every buffered directory **and turns buffering off** for the rest of the
    /// transaction. Used the moment a mutation needs the engine to reflect the current
    /// directory structure — index maintenance reads the engine directly — so no later
    /// write hides a directory in the buffer where that maintenance would miss it.
    pub(in crate::txn) fn take_and_disable(&self) -> HashMap<AKey, Children> {
        self.buffering.store(false, Ordering::Relaxed);

        self.take()
    }
}
