//! The opaque read transaction: a consistent snapshot of one table.

use super::{
    query::IndexQuery,
    grab::{self, Grab},
    rooted::RootedRead,
};

use crate::{
    cache::PathCache,
    codec::ArchivedDir,
    data::{AData, ARef, AValue, Scalar},
    engine::{data_def, entry_split, EntryBytes, INDEX_TABLE, META_TABLE},
    entry::{Entry, EntryKind},
    error::AdbResult,
    index::{
        registry::{self, IndexEntry},
        scan,
        Pattern,
    },
    path::{APath, IntoArborPath, IntoValuePath},
    value::Value,
    AKey,
};

use redb::{ReadOnlyTable, ReadTransaction, ReadableTable, TableError};
use std::sync::{Arc, OnceLock};

#[cfg(feature = "permissions")]
use crate::{
    acl::{AclClass, Rights},
    error::AdbError,
    inode::{decode_meta, read_inode_bytes, Acl, Meta},
    perm::{self, Principal},
};

#[cfg(feature = "entry-timestamps")]
use crate::{
    db::DbInner,
    inode::{read_timestamps, NodeTimestamps, INODES_TABLE},
    time::timestamp_now,
};

#[cfg(feature = "entry-timestamps")]
use std::{collections::HashMap, sync::Mutex};

#[cfg(feature = "permissions")]
use std::collections::HashSet;

/// A read transaction over one table — a consistent, concurrent snapshot.
///
/// Path resolution and hot a-node reads are amortized through the table's shared
/// path cache, tagged with the snapshot's `generation` so a stale entry reads as
/// a miss. The value/filesystem read methods are thin wrappers over the shared
/// `Grab` surface.
pub struct ReadTxn {
    /// The underlying engine read snapshot.
    txn:        ReadTransaction,
    /// The table this snapshot reads.
    table:      Arc<str>,
    /// The table's shared path/blob cache.
    cache:      Arc<PathCache>,
    /// The generation captured at snapshot start, tagging cache lookups.
    generation: u64,
    /// The data table, opened **lazily** on the first read that actually misses the
    /// caches and memoised for the snapshot's lifetime — so a fully cache-served read
    /// opens nothing, while a scan of N entities still opens it at most once. The
    /// inner `None` means the table has never been written.
    data_table: OnceLock<Option<ReadOnlyTable<u128, EntryBytes>>>,

    /// Shared state, held for depositing this snapshot's buffered access times.
    #[cfg(feature = "entry-timestamps")]
    inner: Arc<DbInner>,

    /// Access times recorded by content reads in this snapshot, keyed by a-node.
    /// Deposited into the database-wide log on drop.
    #[cfg(feature = "entry-timestamps")]
    access_log: Mutex<HashMap<AKey, i64>>,

    /// The identity this snapshot reads as; drives ACL enforcement on a protected DB.
    #[cfg(feature = "permissions")]
    principal: Arc<Principal>,

    /// Per-snapshot, per-principal memo of enforced path resolutions. A protected
    /// read cannot use the shared, principal-agnostic path cache, so without this it
    /// re-walks the tree — one inode decode, ACL check and MAC verify per segment —
    /// on every read. It is sound because a read snapshot's data and ACLs are fixed
    /// and this handle's principal never changes, so a path resolved (with its
    /// traversal authorized) once stays so for the snapshot's life.
    #[cfg(feature = "permissions")]
    enforced_paths: Mutex<HashMap<APath, AKey>>,

    /// a-nodes this principal has been granted `Access` on in this snapshot — so a
    /// repeated read of the same target skips the ACL re-check.
    #[cfg(feature = "permissions")]
    access_ok: Mutex<HashSet<AKey>>,

    /// a-nodes whose stored blob has been integrity-verified in this snapshot — so a
    /// repeated read skips recomputing the (keyed-MAC or signature) tag. Keyed by
    /// `AKey` alone is sound: within one snapshot a key names one immutable blob.
    #[cfg(feature = "permissions")]
    verified: Mutex<HashSet<AKey>>,

    /// The `$inodes` table handle, opened lazily and memoised for the snapshot (like
    /// [`data_table`](Self::data_table)) so a protected read opens it at most once.
    #[cfg(feature = "permissions")]
    inodes_table: OnceLock<Option<ReadOnlyTable<&'static [u8], &'static [u8]>>>,
}

impl ReadTxn {
    pub(crate) fn new(
        txn: ReadTransaction,
        table: Arc<str>,
        cache: Arc<PathCache>,
        generation: u64,
        #[cfg(feature = "entry-timestamps")] inner: Arc<DbInner>,
        #[cfg(feature = "permissions")] principal: Arc<Principal>,
    ) -> AdbResult<Self> {
        Ok(Self {
            txn,
            table,
            cache,
            generation,
            // Opened on first cache-missing access (see [`data`](Self::data)), so a
            // fully cache-served read never touches the engine's table registry.
            data_table: OnceLock::new(),
            #[cfg(feature = "entry-timestamps")]
            inner,
            #[cfg(feature = "entry-timestamps")]
            access_log: Mutex::new(HashMap::new()),
            #[cfg(feature = "permissions")]
            principal,
            #[cfg(feature = "permissions")]
            enforced_paths: Mutex::new(HashMap::new()),
            #[cfg(feature = "permissions")]
            access_ok: Mutex::new(HashSet::new()),
            #[cfg(feature = "permissions")]
            verified: Mutex::new(HashSet::new()),
            #[cfg(feature = "permissions")]
            inodes_table: OnceLock::new(),
        })
    }

    /// Whether a-node `akey` has already been `Access`-authorized for this principal
    /// in this snapshot (see [`access_ok`](Self::access_ok)). A poisoned lock reads
    /// as "not yet", so the check simply runs again.
    #[cfg(feature = "permissions")]
    fn access_memoed(&self, akey: AKey) -> bool {
        self.access_ok.lock().map(|seen| seen.contains(&akey)).unwrap_or(false)
    }

    /// Records that this principal holds `Access` on `akey` for this snapshot.
    #[cfg(feature = "permissions")]
    fn memo_access(&self, akey: AKey) {
        if let Ok(mut seen) = self.access_ok.lock() {
            seen.insert(akey);
        }
    }

    /// Whether a-node `akey`'s blob has already been integrity-verified in this
    /// snapshot (see [`verified`](Self::verified)).
    #[cfg(feature = "permissions")]
    fn verify_memoed(&self, akey: AKey) -> bool {
        self.verified.lock().map(|seen| seen.contains(&akey)).unwrap_or(false)
    }

    /// Records that `akey`'s blob has been integrity-verified for this snapshot.
    #[cfg(feature = "permissions")]
    fn memo_verified(&self, akey: AKey) {
        if let Ok(mut seen) = self.verified.lock() {
            seen.insert(akey);
        }
    }

    /// Buffers a content read of `akey` in this snapshot's access log (persisted
    /// later by a committed write or an explicit flush). Best-effort: a poisoned
    /// lock simply skips the record.
    #[cfg(feature = "entry-timestamps")]
    fn buffer_access(&self, akey: AKey) {
        if let Ok(mut log) = self.access_log.lock() {
            log.insert(akey, timestamp_now());
        }
    }

    /// The data table handle, opened lazily on the first cache-missing access and
    /// memoised for the snapshot (see the [`data_table`](Self::data_table) field).
    /// `None` if the table has never been written. A warm read that never calls this
    /// opens no table at all.
    fn data(&self) -> AdbResult<Option<&ReadOnlyTable<u128, EntryBytes>>> {
        if let Some(slot) = self.data_table.get() {
            return Ok(slot.as_ref());
        }

        let opened = match self.txn.open_table(data_def(&self.table)) {
            Ok(table) => Some(table),
            Err(TableError::TableDoesNotExist(_)) => None,
            Err(err) => return Err(err.into()),
        };

        // On a race another thread may have set it first; keep whichever won.
        let _ = self.data_table.set(opened);

        Ok(self
            .data_table
            .get()
            .expect("the data table slot was just set")
            .as_ref())
    }

    /// The entry blob for `akey`, served from the blob cache or read once from the
    /// engine (and then cached). `None` if the a-node is absent.
    fn blob_at<R>(&self, table: &R, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        if let Some(blob) = self.cache.get_blob(self.generation, akey)? {
            return Ok(Some(blob));
        }

        match table.get(u128::from(akey))? {
            Some(guard) => {
                let blob = Arc::new(guard.value().to_vec());
                self.cache.put_blob(self.generation, akey, Arc::clone(&blob))?;

                Ok(Some(blob))
            }
            None => Ok(None),
        }
    }

    /// Whether ACL enforcement applies — a protected database read as a real
    /// principal (guest or an authenticated user), not an unrestricted handle.
    #[cfg(feature = "permissions")]
    fn enforced(&self) -> bool {
        !matches!(self.principal.as_ref(), Principal::Unrestricted)
    }

    /// The per-a-node metadata table, opened lazily on first protected access and
    /// memoised for the snapshot. `None` if it does not exist yet (a database with no
    /// `$inodes` has no ACLs — every a-node reads as `None`).
    #[cfg(feature = "permissions")]
    fn inodes(&self) -> AdbResult<Option<&ReadOnlyTable<&'static [u8], &'static [u8]>>> {
        if let Some(slot) = self.inodes_table.get() {
            return Ok(slot.as_ref());
        }

        let opened = match self.txn.open_table(INODES_TABLE) {
            Ok(table) => Some(table),
            Err(TableError::TableDoesNotExist(_)) => None,
            Err(err) => return Err(err.into()),
        };

        let _ = self.inodes_table.set(opened);

        Ok(self
            .inodes_table
            .get()
            .expect("the inode table slot was just set")
            .as_ref())
    }

    /// A-node `akey`'s decoded metadata (ACL + both integrity tags), served from the
    /// shared inode-bytes cache or read once from `$inodes` and then cached. The
    /// cached bytes are principal-independent; the ACL check and integrity verify that
    /// consume this still run per read on the reading principal. All-`None` if the
    /// a-node has no inode yet.
    #[cfg(feature = "permissions")]
    fn meta(&self, akey: AKey) -> AdbResult<Meta> {
        // Warm path: decode the cached bytes without touching the engine.
        if let Some(bytes) = self.cache.get_inode(self.generation, akey)? {
            return decode_meta(&bytes);
        }

        let Some(inodes) = self.inodes()? else {
            return Ok((None, None, None));
        };

        match read_inode_bytes(inodes, &self.table, akey)? {
            Some(bytes) => {
                let bytes = Arc::new(bytes);
                self.cache.put_inode(self.generation, akey, Arc::clone(&bytes))?;

                decode_meta(&bytes)
            }
            None => Ok((None, None, None)),
        }
    }

    /// Checks the `needed` grade on `akey` against its ACL. A no-op when enforcement
    /// is off.
    #[cfg(feature = "permissions")]
    fn authorize_target(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        if !self.enforced() {
            return Ok(());
        }

        let (acl, ..) = self.meta(akey)?;

        perm::access::authorize(&self.principal, akey, acl.as_ref(), needed)
    }

    /// Checks a-node `akey`'s integrity tag over `blob`, given metadata the caller has
    /// **already decoded** (its ACL and both tags, read once). An authenticated
    /// reader checks the fast keyed MAC; a keyless guest checks the Ed25519 signature
    /// with the public key; an unrestricted handle has nothing to verify.
    #[cfg(feature = "permissions")]
    fn check_tag(
        &self,
        akey: AKey,
        blob: &[u8],
        acl: Option<&Acl>,
        mac: Option<[u8; 32]>,
        sig: Option<[u8; crate::crypto::SIG_LEN]>,
    ) -> AdbResult<()> {
        match self.principal.as_ref() {
            // An authenticated reader verifies the fast keyed MAC.
            Principal::User(session) => {
                // A blob MAC-verified earlier this generation needs no re-check:
                // within a generation the snapshot is immutable (redb MVCC), so the
                // same bytes would give the same result, and the MAC is keyed by the
                // database-global integrity key (so the outcome is principal-independent
                // and safe to share). The cache is empty after a reopen, so at-rest
                // tampering is still caught on the first read.
                if self.cache.get_mac_verified(self.generation, akey)? {
                    return Ok(());
                }

                let acl = acl.map(|acl| acl.encode()).unwrap_or_default();
                let expected = perm::integrity::mac_value(session.key(), &self.table, akey, blob, &acl);

                match mac {
                    Some(mac) if perm::integrity::ct_eq(&mac, &expected) => {
                        self.cache.put_mac_verified(self.generation, akey)?;

                        Ok(())
                    }
                    _ => Err(self.tampered()),
                }
            }

            // A keyless guest verifies the Ed25519 signature with the public key: it
            // can check integrity without being able to forge (seal) a value.
            Principal::Guest {
                pubkey,
            } => {
                let acl = acl.map(|acl| acl.encode()).unwrap_or_default();

                match sig {
                    Some(sig) if perm::integrity::verify_value(pubkey, &self.table, akey, blob, &acl, &sig) => Ok(()),
                    _ => Err(self.tampered()),
                }
            }

            // An unrestricted handle (a non-protected database) has nothing to verify.
            Principal::Unrestricted => Ok(()),
        }
    }

    /// The tamper error naming this snapshot's table.
    #[cfg(feature = "permissions")]
    fn tampered(&self) -> AdbError {
        AdbError::Tampered(format!(
            "integrity check failed for an a-node in table '{}'",
            self.table
        ))
    }

    /// Verifies a-node `akey`'s integrity tag over `blob` — the keyed MAC for an
    /// authenticated user, the signature for a guest. A no-op for an unrestricted
    /// handle (a non-protected database has no tags). The ACL and tag come from one
    /// (cache-served) inode decode.
    #[cfg(feature = "permissions")]
    fn verify_integrity(&self, akey: AKey, blob: &[u8]) -> AdbResult<()> {
        if matches!(self.principal.as_ref(), Principal::Unrestricted) {
            return Ok(());
        }

        let (acl, mac, sig) = self.meta(akey)?;

        self.check_tag(akey, blob, acl.as_ref(), mac, sig)
    }

    /// Resolves `path` with a `walk` check on every directory traversed, bypassing
    /// the shared path cache (which is not principal-scoped). `None` if a component
    /// along the way is missing.
    #[cfg(feature = "permissions")]
    fn resolve_enforced<R>(&self, table: &R, path: &APath) -> AdbResult<Option<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let mut akey = AKey::ROOT;
        for name in path.names() {
            // One (cache-served) inode fetch per directory: the ACL authorizes the
            // traversal and, with a tag, verifies the directory blob just below.
            let (acl, mac, sig) = self.meta(akey)?;

            perm::access::authorize(&self.principal, akey, acl.as_ref(), Rights::Access)?;

            let Some(blob) = self.blob_at(table, akey)? else {
                return Ok(None);
            };

            // A tampered directory blob could redirect a name to another a-node, so
            // verify each directory descended through (for a keyed principal).
            self.check_tag(akey, &blob, acl.as_ref(), mac, sig)?;

            let (kind, payload) = entry_split(&blob)?;
            if kind != EntryKind::Dir {
                return Ok(None);
            }

            match ArchivedDir::new(payload)?.get(name.as_str())? {
                Some(child) => akey = child,
                None => return Ok(None),
            }
        }

        Ok(Some(akey))
    }

    /// Walks the directory tree from the root to resolve `path`, caching the result.
    /// The caller has already consulted the path cache (a miss) and opened `table`;
    /// this is the un-enforced walk (a protected database uses
    /// [`resolve_enforced`](Self::resolve_enforced) instead).
    fn resolve_walk<R>(&self, table: &R, path: &APath) -> AdbResult<Option<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let mut akey = AKey::ROOT;
        for name in path.names() {
            let Some(blob) = self.blob_at(table, akey)? else {
                return Ok(None);
            };

            let (kind, payload) = entry_split(&blob)?;
            if kind != EntryKind::Dir {
                return Ok(None);
            }

            match ArchivedDir::new(payload)?.get(name.as_str())? {
                Some(child) => akey = child,
                None => return Ok(None),
            }
        }

        self.cache.put_path(self.generation, path.clone(), akey)?;

        Ok(Some(akey))
    }

    /// Loads a typed value from the file at `path`, or `None` if absent. Errors if
    /// `path` names a directory.
    pub fn load<T: AData>(&self, path: impl IntoArborPath) -> AdbResult<Option<T>> {
        grab::load(self, path)
    }

    /// Loads a file stored at `path` into any [`serde::de::DeserializeOwned`] value,
    /// reading the value blob **directly** over the zero-copy codec — no intermediate
    /// [`Value`] tree — the counterpart of
    /// [`WriteTxn::store_serde_value`](crate::txn::WriteTxn::store_serde_value).
    /// `None` if absent; errors if `path` names a directory.
    #[cfg(feature = "serde")]
    pub fn load_serde_value<T: serde::de::DeserializeOwned>(&self, path: impl IntoArborPath) -> AdbResult<Option<T>> {
        grab::load_serde_value(self, path)
    }

    /// Loads the whole dynamic [`Value`] stored in the file at `path`, or `None` if
    /// there is nothing there. Errors if `path` names a directory.
    pub fn load_value(&self, path: impl IntoArborPath) -> AdbResult<Option<Value>> {
        grab::load_value(self, path)
    }

    /// Opens a read accessor over the file at `path`, navigating its value blob
    /// zero-copy. `None` if the file is absent; errors if `path` names a directory.
    /// The accessor owns a snapshot of the blob, so it may outlive the transaction.
    pub fn fetch<A: ARef<'static>>(&self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        grab::fetch(self, path)
    }

    /// Reads the scalar at `at` inside the file at `path`, navigating the value
    /// blob zero-copy. `None` if the file or the inner path is absent, or if the
    /// inner path does not land on a scalar leaf.
    pub fn get(&self, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<Scalar>> {
        grab::get(self, path, at)
    }

    /// Reads a typed scalar at `at` inside the file at `path`.
    pub fn get_as<V: AValue>(&self, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<V>> {
        grab::get_as(self, path, at)
    }

    /// The filesystem kind (file or directory) at `path`, or `None` if absent.
    pub fn kind(&self, path: impl IntoArborPath) -> AdbResult<Option<EntryKind>> {
        grab::kind(self, path)
    }

    /// Whether a file or directory exists at `path`.
    pub fn exists(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        grab::exists(self, path)
    }

    /// Lists the direct children of the directory at `path`, as `(name, kind)`
    /// pairs in name order. Errors if `path` names a file.
    pub fn ls(&self, path: impl IntoArborPath) -> AdbResult<Vec<Entry>> {
        grab::ls(self, path)
    }

    /// The created / modified / accessed timestamps of the a-node at `path`, or
    /// `None` if the a-node is absent or has no recorded metadata yet (for instance
    /// an a-node last written by a binary built without `entry-timestamps`).
    ///
    /// The `accessed` field reflects committed access times; a read in the current
    /// snapshot is buffered and persisted later (see the crate docs), so it may lag.
    #[cfg(feature = "entry-timestamps")]
    pub fn times(&self, path: impl IntoArborPath) -> AdbResult<Option<NodeTimestamps>> {
        grab::times(self, path)
    }

    /// The [`Rights`] the file or directory at `path` grants `class`.
    ///
    /// Returns [`Rights::None`] when the a-node has no ACL (a non-protected database,
    /// the special root, or a node predating protection) or when a
    /// [`Group`](AclClass::Group) names one the a-node is not in. Errors with
    /// [`ValueNotFound`](AdbError::ValueNotFound) if nothing exists at `path`. Read
    /// the owner's name and the group list with [`owner`](Self::owner) and
    /// [`groups`](Self::groups).
    #[cfg(feature = "permissions")]
    pub fn get_acl(&self, path: impl IntoArborPath, class: AclClass) -> AdbResult<Rights> {
        grab::get_acl(self, path, class)
    }

    /// The name of the owner of the file or directory at `path`, or `None` if the
    /// a-node is absent or has no ACL. Falls back to the numeric id if the owning user
    /// is no longer in the store.
    #[cfg(feature = "permissions")]
    pub fn owner(&self, path: impl IntoArborPath) -> AdbResult<Option<String>> {
        grab::owner(self, path)
    }

    /// The names of the groups the file or directory at `path` belongs to, sorted;
    /// empty when the a-node is absent, has no ACL, or is in no group.
    #[cfg(feature = "permissions")]
    pub fn groups(&self, path: impl IntoArborPath) -> AdbResult<Vec<String>> {
        grab::groups(self, path)
    }

    /// Finds the entities an index points at, recomposing each as a `T`.
    ///
    /// `values` are matched against the index's leading columns (in column order):
    /// the full set is an exact lookup, fewer is a prefix lookup, and an empty
    /// slice matches every indexed entity. Results come back in index order
    /// (ascending by the encoded key, honoring each column's ASC/DESC). For reverse
    /// order or a subtree scope use [`query`](Self::query). Errors with
    /// [`IndexNotFound`](crate::AdbError::IndexNotFound) for an unknown index and
    /// [`IndexArity`](crate::AdbError::IndexArity) for more values than the index has columns.
    pub fn find<T: AData>(&self, index: &str, values: &[Scalar]) -> AdbResult<Vec<T>> {
        grab::find(self, index, values)
    }

    /// Starts an [`IndexQuery`] against `index` — a builder for prefix matches,
    /// reverse order, and subtree scoping.
    pub fn query(&self, index: &str) -> IndexQuery<'_> {
        IndexQuery::new(self, index)
    }

    /// A view of this transaction whose access paths are relative to `root`.
    pub fn rooted(&self, root: impl IntoArborPath) -> AdbResult<RootedRead<'_>> {
        Ok(RootedRead::new(self, root.into_arbor_path()?))
    }
}

impl Grab for ReadTxn {
    fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
        // A protected database resolves with per-directory access checks and cannot
        // use the (principal-agnostic) shared path cache, so it needs the table first.
        #[cfg(feature = "permissions")]
        if self.enforced() {
            // A resolution memoed earlier this snapshot skips the re-walk and its
            // per-segment inode reads, ACL checks and MAC verifies.
            if let Ok(memo) = self.enforced_paths.lock()
                && let Some(&akey) = memo.get(path)
            {
                return Ok(Some(akey));
            }

            let Some(table) = self.data()? else {
                return Ok(path.is_root().then_some(AKey::ROOT));
            };

            let resolved = self.resolve_enforced(table, path)?;
            if let Some(akey) = resolved
                && let Ok(mut memo) = self.enforced_paths.lock()
            {
                memo.insert(path.clone(), akey);
            }

            return Ok(resolved);
        }

        // Warm path: a cached resolution is returned without opening the data table.
        if let Some(akey) = self.cache.get_path(self.generation, path)? {
            return Ok(Some(akey));
        }

        // Miss: the root always resolves even before the table is materialized; any
        // named component needs the table.
        let Some(table) = self.data()? else {
            return Ok(path.is_root().then_some(AKey::ROOT));
        };

        self.resolve_walk(table, path)
    }

    fn entry_blob(&self, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>> {
        // Warm path: a cached blob is served without opening the data table.
        if let Some(blob) = self.cache.get_blob(self.generation, akey)? {
            return Ok(Some(blob));
        }

        match self.data()? {
            Some(table) => self.blob_at(table, akey),
            None => Ok(None),
        }
    }

    fn lookup_index(&self, index: &str) -> AdbResult<Option<IndexEntry>> {
        let meta = self.txn.open_table(META_TABLE)?;

        registry::lookup(&meta, &self.table, index)
    }

    fn scan_index(&self, entry: &IndexEntry, cols: &[u8]) -> AdbResult<Vec<AKey>> {
        let index_table = match self.txn.open_table(INDEX_TABLE) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        scan::scan_prefix(&index_table, entry.id(), cols, entry.def().unique())
    }

    fn affected_under(&self, pattern: &Pattern, root: &APath) -> AdbResult<Vec<AKey>> {
        match self.data()? {
            Some(data) => pattern.affected_entities(data, root),
            None => Ok(Vec::new()),
        }
    }

    #[cfg(feature = "permissions")]
    fn authorize(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        if !self.enforced() {
            return Ok(());
        }

        // Reads only ever need `Access`, so memoing that one grade covers them; any
        // stronger grade always re-checks (it never reaches here on a read).
        if needed == Rights::Access {
            if self.access_memoed(akey) {
                return Ok(());
            }

            self.authorize_target(akey, needed)?;
            self.memo_access(akey);

            return Ok(());
        }

        self.authorize_target(akey, needed)
    }

    #[cfg(feature = "permissions")]
    fn verify(&self, akey: AKey, blob: &[u8]) -> AdbResult<()> {
        // An unrestricted handle has nothing to verify (and no memo to keep).
        if matches!(self.principal.as_ref(), Principal::Unrestricted) {
            return Ok(());
        }

        if self.verify_memoed(akey) {
            return Ok(());
        }

        self.verify_integrity(akey, blob)?;
        self.memo_verified(akey);

        Ok(())
    }

    #[cfg(feature = "permissions")]
    fn acl_of(&self, akey: AKey) -> AdbResult<Option<Acl>> {
        let (acl, ..) = self.meta(akey)?;

        Ok(acl)
    }

    #[cfg(feature = "permissions")]
    fn gid_of(&self, group: &str) -> AdbResult<Option<u32>> {
        let meta = self.txn.open_table(META_TABLE)?;

        perm::store::gid_of(&meta, group)
    }

    #[cfg(feature = "permissions")]
    fn user_name(&self, uid: u32) -> AdbResult<Option<String>> {
        let meta = self.txn.open_table(META_TABLE)?;

        perm::store::name_of_user(&meta, uid)
    }

    #[cfg(feature = "permissions")]
    fn group_name(&self, gid: u32) -> AdbResult<Option<String>> {
        let meta = self.txn.open_table(META_TABLE)?;

        perm::store::name_of_group(&meta, gid)
    }

    #[cfg(feature = "entry-timestamps")]
    fn record_access(&self, akey: AKey) {
        self.buffer_access(akey);
    }

    #[cfg(feature = "entry-timestamps")]
    fn timestamps_of(&self, akey: AKey) -> AdbResult<Option<NodeTimestamps>> {
        let inodes = match self.txn.open_table(INODES_TABLE) {
            Ok(inodes) => inodes,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        read_timestamps(&inodes, &self.table, akey)
    }
}

#[cfg(feature = "entry-timestamps")]
impl Drop for ReadTxn {
    /// Deposits this snapshot's buffered access times into the database-wide log,
    /// to be persisted on the next committed write or explicit flush.
    fn drop(&mut self) {
        let entries: Vec<(AKey, i64)> = match self.access_log.lock() {
            Ok(mut log) => log.drain().collect(),
            Err(_) => return,
        };

        self.inner.deposit_access(&self.table, entries);
    }
}
