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
use std::sync::Arc;

#[cfg(feature = "permissions")]
use crate::{
    acl::{AclClass, Rights},
    error::AdbError,
    inode::{read_acl, read_mac, read_sig, Acl},
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

/// A read transaction over one table — a consistent, concurrent snapshot.
///
/// Path resolution and hot-vnode reads are amortized through the table's shared
/// path cache, tagged with the snapshot's `generation` so a stale entry reads as
/// a miss. The value/filesystem read methods are thin wrappers over the shared
/// `Grab` surface.
pub struct ReadTxn {
    /// The underlying engine read snapshot.
    txn:        ReadTransaction,
    /// The table this snapshot reads.
    table:      String,
    /// The table's shared path/blob cache.
    cache:      Arc<PathCache>,
    /// The generation captured at snapshot start, tagging cache lookups.
    generation: u64,

    /// Shared state, held for depositing this snapshot's buffered access times.
    #[cfg(feature = "entry-timestamps")]
    inner: Arc<DbInner>,

    /// Access times recorded by content reads in this snapshot, keyed by vnode.
    /// Deposited into the database-wide log on drop.
    #[cfg(feature = "entry-timestamps")]
    access_log: Mutex<HashMap<AKey, i64>>,

    /// The identity this snapshot reads as; drives ACL enforcement on a protected DB.
    #[cfg(feature = "permissions")]
    principal: Arc<Principal>,
}

impl ReadTxn {
    pub(crate) fn new(
        txn: ReadTransaction,
        table: String,
        cache: Arc<PathCache>,
        generation: u64,
        #[cfg(feature = "entry-timestamps")] inner: Arc<DbInner>,
        #[cfg(feature = "permissions")] principal: Arc<Principal>,
    ) -> Self {
        Self {
            txn,
            table,
            cache,
            generation,
            #[cfg(feature = "entry-timestamps")]
            inner,
            #[cfg(feature = "entry-timestamps")]
            access_log: Mutex::new(HashMap::new()),
            #[cfg(feature = "permissions")]
            principal,
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

    /// Opens the data table, or `None` if it has never been written.
    fn open(&self) -> AdbResult<Option<ReadOnlyTable<u128, EntryBytes>>> {
        match self.txn.open_table(data_def(&self.table)) {
            Ok(table) => Ok(Some(table)),
            Err(TableError::TableDoesNotExist(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// The entry blob for `akey`, served from the blob cache or read once from the
    /// engine (and then cached). `None` if the vnode is absent.
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

    /// Opens the per-vnode metadata table read-only, or `None` if it does not exist
    /// yet (a database with no `$inodes` has no ACLs — every vnode reads as `None`).
    #[cfg(feature = "permissions")]
    fn open_inodes(&self) -> AdbResult<Option<ReadOnlyTable<&'static [u8], &'static [u8]>>> {
        match self.txn.open_table(INODES_TABLE) {
            Ok(table) => Ok(Some(table)),
            Err(TableError::TableDoesNotExist(_)) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Checks the `needed` grade on `akey` against its ACL. A no-op when enforcement
    /// is off.
    #[cfg(feature = "permissions")]
    fn authorize_target(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        if !self.enforced() {
            return Ok(());
        }

        let inodes = self.open_inodes()?;
        let acl = match &inodes {
            Some(table) => read_acl(table, &self.table, akey)?,
            None => None,
        };

        perm::access::authorize(&self.principal, akey, acl.as_ref(), needed)
    }

    /// Verifies vnode `akey`'s integrity tag over `blob` and its ACL, using an
    /// already-open `$inodes` handle. A no-op unless the reader holds the integrity
    /// key: only an authenticated user can verify — the guest has no key, so a guest
    /// read is unverified (and the guest is read-only anyway).
    #[cfg(feature = "permissions")]
    fn verify_blob(
        &self,
        inodes: Option<&ReadOnlyTable<&'static [u8], &'static [u8]>>,
        akey: AKey,
        blob: &[u8],
    ) -> AdbResult<()> {
        // Both tags bind the ACL, so read it once for whichever check the principal
        // runs. (An unrestricted handle never reaches here — it resolves unenforced.)
        let acl = match inodes {
            Some(table) => read_acl(table, &self.table, akey)?
                .map(|acl| acl.encode())
                .unwrap_or_default(),
            None => Vec::new(),
        };

        match self.principal.as_ref() {
            // An authenticated reader verifies the fast keyed MAC.
            Principal::User(session) => {
                let stored = inodes
                    .map(|table| read_mac(table, &self.table, akey))
                    .transpose()?
                    .flatten();
                let expected = perm::integrity::mac_value(session.key(), &self.table, akey, blob, &acl);

                match stored {
                    Some(mac) if perm::integrity::ct_eq(&mac, &expected) => Ok(()),
                    _ => Err(self.tampered()),
                }
            }

            // A keyless guest verifies the Ed25519 signature with the public key: it
            // can check integrity without being able to forge (seal) a value.
            Principal::Guest {
                pubkey,
            } => {
                let stored = inodes
                    .map(|table| read_sig(table, &self.table, akey))
                    .transpose()?
                    .flatten();

                match stored {
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
        AdbError::Tampered(format!("integrity check failed for a vnode in table '{}'", self.table))
    }

    /// Opens `$inodes` and verifies vnode `akey`'s integrity tag over `blob` — the
    /// keyed MAC for an authenticated user, the signature for a guest. A no-op for an
    /// unrestricted handle (a non-protected database has no tags).
    #[cfg(feature = "permissions")]
    fn verify_integrity(&self, akey: AKey, blob: &[u8]) -> AdbResult<()> {
        if matches!(self.principal.as_ref(), Principal::Unrestricted) {
            return Ok(());
        }

        let inodes = self.open_inodes()?;
        self.verify_blob(inodes.as_ref(), akey, blob)
    }

    /// Resolves `path` with a `walk` check on every directory traversed, bypassing
    /// the shared path cache (which is not principal-scoped). `None` if a component
    /// along the way is missing.
    #[cfg(feature = "permissions")]
    fn resolve_enforced<R>(&self, table: &R, path: &APath) -> AdbResult<Option<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let inodes = self.open_inodes()?;

        let mut akey = AKey::ROOT;
        for name in path.names() {
            let acl = match &inodes {
                Some(t) => read_acl(t, &self.table, akey)?,
                None => None,
            };

            perm::access::authorize(&self.principal, akey, acl.as_ref(), Rights::Access)?;

            let Some(blob) = self.blob_at(table, akey)? else {
                return Ok(None);
            };

            // A tampered directory blob could redirect a name to another vnode, so
            // verify each directory descended through (for a keyed principal).
            self.verify_blob(inodes.as_ref(), akey, &blob)?;

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

    /// Resolves `path` to a vnode key. On a protected database this walks the tree
    /// with `walk` checks (bypassing the shared path cache); otherwise it consults
    /// the path cache first and walks (through the blob cache) on a miss.
    fn resolve_cached<R>(&self, table: &R, path: &APath) -> AdbResult<Option<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        #[cfg(feature = "permissions")]
        if self.enforced() {
            return self.resolve_enforced(table, path);
        }

        if let Some(akey) = self.cache.get_path(self.generation, path)? {
            return Ok(Some(akey));
        }

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

    /// The created / modified / accessed timestamps of the vnode at `path`, or
    /// `None` if the vnode is absent or has no recorded metadata yet (for instance
    /// a vnode last written by a binary built without `entry-timestamps`).
    ///
    /// The `accessed` field reflects committed access times; a read in the current
    /// snapshot is buffered and persisted later (see the crate docs), so it may lag.
    #[cfg(feature = "entry-timestamps")]
    pub fn times(&self, path: impl IntoArborPath) -> AdbResult<Option<NodeTimestamps>> {
        grab::times(self, path)
    }

    /// The [`Rights`] the file or directory at `path` grants `class`.
    ///
    /// Returns [`Rights::None`] when the vnode has no ACL (a non-protected database,
    /// the special root, or a node predating protection) or when a
    /// [`Group`](AclClass::Group) names one the vnode is not in. Errors with
    /// [`ValueNotFound`](AdbError::ValueNotFound) if nothing exists at `path`. Read
    /// the owner's name and the group list with [`owner`](Self::owner) and
    /// [`groups`](Self::groups).
    #[cfg(feature = "permissions")]
    pub fn get_acl(&self, path: impl IntoArborPath, class: AclClass) -> AdbResult<Rights> {
        grab::get_acl(self, path, class)
    }

    /// The name of the owner of the file or directory at `path`, or `None` if the
    /// vnode is absent or has no ACL. Falls back to the numeric id if the owning user
    /// is no longer in the store.
    #[cfg(feature = "permissions")]
    pub fn owner(&self, path: impl IntoArborPath) -> AdbResult<Option<String>> {
        grab::owner(self, path)
    }

    /// The names of the groups the file or directory at `path` belongs to, sorted;
    /// empty when the vnode is absent, has no ACL, or is in no group.
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
        let Some(table) = self.open()? else {
            // The root always resolves, even before the data table is materialized;
            // any named component needs the table.
            return Ok(path.is_root().then_some(AKey::ROOT));
        };

        self.resolve_cached(&table, path)
    }

    fn entry_blob(&self, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>> {
        match self.open()? {
            Some(table) => self.blob_at(&table, akey),
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
        match self.open()? {
            Some(data) => pattern.affected_entities(&data, root),
            None => Ok(Vec::new()),
        }
    }

    #[cfg(feature = "permissions")]
    fn authorize(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        self.authorize_target(akey, needed)
    }

    #[cfg(feature = "permissions")]
    fn verify(&self, akey: AKey, blob: &[u8]) -> AdbResult<()> {
        self.verify_integrity(akey, blob)
    }

    #[cfg(feature = "permissions")]
    fn acl_of(&self, akey: AKey) -> AdbResult<Option<Acl>> {
        match self.open_inodes()? {
            Some(inodes) => read_acl(&inodes, &self.table, akey),
            None => Ok(None),
        }
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
