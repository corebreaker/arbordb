//! The opaque read transaction: a consistent snapshot of one table.

use super::{query::IndexQuery, rooted::RootedRead};
use crate::{
    access::{ArchivedReader, Reader},
    cache::PathCache,
    codec::{decode, ArchivedDir, ArchivedValue},
    data::{AData, ARef, AValue, Scalar},
    engine::{data_def, get_entry_kind, entry_split, EntryBytes, INDEX_TABLE, META_TABLE},
    entry::{Entry, EntryKind},
    error::{AdbError, AdbResult},
    index::{registry, scan, Pattern},
    vnode::NodeKind,
    path::{APath, IntoArborPath, IntoValuePath, VPath},
    value::Value,
    AKey,
};

use redb::{ReadOnlyTable, ReadTransaction, ReadableTable, TableError};
use std::{collections::HashSet, sync::Arc};

#[cfg(feature = "entry-timestamps")]
use crate::{db::DbInner, time::timestamp_now};

#[cfg(feature = "entry-timestamps")]
use std::{collections::HashMap, sync::Mutex};

#[cfg(feature = "permissions")]
use crate::{
    inode::{read_acl, read_mac, Right, INODES_TABLE},
    perm::{authorize, ct_eq, mac_value, Principal},
};

/// A read transaction over one table — a consistent, concurrent snapshot.
///
/// Path resolution and hot-vnode reads are amortized through the table's shared
/// path cache, tagged with the snapshot's `generation` so a stale entry reads as
/// a miss.
pub struct ReadTxn {
    txn:        ReadTransaction,
    table:      String,
    cache:      Arc<PathCache>,
    generation: u64,

    // Shared state, held for depositing this snapshot's buffered access times.
    #[cfg(feature = "entry-timestamps")]
    inner: Arc<DbInner>,

    // Access times recorded by content reads in this snapshot, keyed by vnode.
    // Deposited into the database-wide log on drop.
    #[cfg(feature = "entry-timestamps")]
    access_log: Mutex<HashMap<AKey, i64>>,

    // The identity this snapshot reads as; drives ACL enforcement on a protected DB.
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
    fn record_access(&self, akey: AKey) {
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
    fn entry_blob<R>(&self, table: &R, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>>
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

    /// Checks `right` on `akey` against its ACL. A no-op when enforcement is off.
    #[cfg(feature = "permissions")]
    fn authorize_target(&self, akey: AKey, right: Right) -> AdbResult<()> {
        if !self.enforced() {
            return Ok(());
        }

        let inodes = self.open_inodes()?;
        let acl = match &inodes {
            Some(table) => read_acl(table, &self.table, akey)?,
            None => None,
        };

        authorize(&self.principal, akey, acl.as_ref(), right)
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
        let Principal::User(session) = self.principal.as_ref() else {
            return Ok(());
        };

        let (stored, acl) = match inodes {
            Some(table) => (
                read_mac(table, &self.table, akey)?,
                read_acl(table, &self.table, akey)?
                    .map(|acl| acl.encode())
                    .unwrap_or_default(),
            ),
            None => (None, Vec::new()),
        };

        let expected = mac_value(session.key(), &self.table, akey, blob, &acl);
        match stored {
            Some(mac) if ct_eq(&mac, &expected) => Ok(()),
            _ => Err(AdbError::Tampered(format!(
                "integrity check failed for a vnode in table '{}'",
                self.table
            ))),
        }
    }

    /// Opens `$inodes` and verifies vnode `akey`'s integrity tag over `blob`. A
    /// no-op for an unrestricted handle or the keyless guest.
    #[cfg(feature = "permissions")]
    fn verify_integrity(&self, akey: AKey, blob: &[u8]) -> AdbResult<()> {
        if !matches!(self.principal.as_ref(), Principal::User(_)) {
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
            authorize(&self.principal, akey, acl.as_ref(), Right::Walk)?;

            let Some(blob) = self.entry_blob(table, akey)? else {
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
            let Some(blob) = self.entry_blob(table, akey)? else {
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

    /// Loads the whole dynamic [`Value`] stored in the file at `path`, or `None` if
    /// there is nothing there. Errors if `path` names a directory.
    pub fn load_value(&self, path: impl IntoArborPath) -> AdbResult<Option<Value>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.authorize_target(akey, Right::Read)?;

        #[cfg(feature = "entry-timestamps")]
        self.record_access(akey);

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_integrity(akey, &blob)?;

        let (kind, payload) = entry_split(&blob)?;
        match kind {
            EntryKind::File => Ok(Some(decode(payload)?)),
            EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
        }
    }

    /// Loads a typed value from the file at `path`, or `None` if absent. Errors if
    /// `path` names a directory.
    pub fn load<T: AData>(&self, path: impl IntoArborPath) -> AdbResult<Option<T>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.authorize_target(akey, Right::Read)?;

        #[cfg(feature = "entry-timestamps")]
        self.record_access(akey);

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_integrity(akey, &blob)?;

        match get_entry_kind(&blob)? {
            EntryKind::File => {
                // Skip the one-byte entry tag; the value payload starts at offset 1.
                let reader = ArchivedReader::new(blob, 1);

                Ok(Some(T::load(&reader, &VPath::root())?))
            }
            EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
        }
    }

    /// Opens a read accessor over the file at `path`, navigating its value blob
    /// zero-copy. `None` if the file is absent; errors if `path` names a directory.
    /// The accessor owns a snapshot of the blob, so it may outlive the transaction.
    pub fn fetch<A: ARef<'static>>(&self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.authorize_target(akey, Right::Read)?;

        #[cfg(feature = "entry-timestamps")]
        self.record_access(akey);

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_integrity(akey, &blob)?;

        match get_entry_kind(&blob)? {
            EntryKind::File => {
                let reader: Arc<dyn Reader> = Arc::new(ArchivedReader::new(blob, 1));

                Ok(Some(A::open(reader, VPath::root())))
            }
            EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
        }
    }

    /// Reads the scalar at `at` inside the file at `path`, navigating the value
    /// blob zero-copy. `None` if the file or the inner path is absent, or if the
    /// inner path does not land on a scalar leaf.
    pub fn get(&self, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<Scalar>> {
        let path = path.into_arbor_path()?;
        let at = at.into_value_path()?;

        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.authorize_target(akey, Right::Read)?;

        #[cfg(feature = "entry-timestamps")]
        self.record_access(akey);

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_integrity(akey, &blob)?;

        let (kind, payload) = entry_split(&blob)?;
        if kind != EntryKind::File {
            return Ok(None);
        }

        let Some(node) = ArchivedValue::new(payload)?.root().navigate(&at)? else {
            return Ok(None);
        };

        match node.kind()? {
            NodeKind::Leaf => Ok(Some(node.scalar()?)),
            _ => Ok(None),
        }
    }

    /// Reads a typed scalar at `at` inside the file at `path`.
    pub fn get_as<V: AValue>(&self, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<V>> {
        match self.get(path, at)? {
            Some(scalar) => Ok(Some(V::from_scalar(&scalar)?)),
            None => Ok(None),
        }
    }

    /// The filesystem kind (file or directory) at `path`, or `None` if absent.
    pub fn kind(&self, path: impl IntoArborPath) -> AdbResult<Option<EntryKind>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        match self.entry_blob(&table, akey)? {
            Some(blob) => Ok(Some(get_entry_kind(&blob)?)),
            None => Ok(None),
        }
    }

    /// Whether a file or directory exists at `path`.
    pub fn exists(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        Ok(self.kind(path)?.is_some())
    }

    /// The created / modified / accessed timestamps of the vnode at `path`, or
    /// `None` if the vnode is absent or has no recorded metadata yet (for instance
    /// a vnode last written by a binary built without `entry-timestamps`).
    ///
    /// The `accessed` field reflects committed access times; a read in the current
    /// snapshot is buffered and persisted later (see the crate docs), so it may lag.
    #[cfg(feature = "entry-timestamps")]
    pub fn times(&self, path: impl IntoArborPath) -> AdbResult<Option<crate::inode::NodeTimestamps>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        let inodes = match self.txn.open_table(crate::inode::INODES_TABLE) {
            Ok(inodes) => inodes,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        crate::inode::read_times(&inodes, &self.table, akey)
    }

    /// The access-control list of the file or directory at `path`, with owner and
    /// group resolved to names — or `None` if the node is absent or has no ACL (a
    /// non-protected database, the special root, or a node predating protection).
    #[cfg(feature = "permissions")]
    pub fn get_acl(&self, path: impl IntoArborPath) -> AdbResult<Option<crate::acl::NodeAcl>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        let Some(inodes) = self.open_inodes()? else {
            return Ok(None);
        };
        let Some(acl) = read_acl(&inodes, &self.table, akey)? else {
            return Ok(None);
        };

        let meta = self.txn.open_table(META_TABLE)?;
        let owner = crate::perm::name_of_user(&meta, acl.owner())?.unwrap_or_else(|| acl.owner().to_string());
        let group = match acl.group() {
            Some(gid) => crate::perm::name_of_group(&meta, gid)?,
            None => None,
        };

        Ok(Some(crate::acl::NodeAcl {
            owner,
            group,
            mode: acl.to_mode(),
        }))
    }

    /// Finds the entities an index points at, recomposing each as a `T`.
    ///
    /// `values` are matched against the index's leading columns (in column order):
    /// the full set is an exact lookup, fewer is a prefix lookup, and an empty
    /// slice matches every indexed entity. Results come back in index order
    /// (ascending by the encoded key, honoring each column's ASC/DESC). For reverse
    /// order or a subtree scope use [`query`](Self::query). Errors with
    /// [`IndexNotFound`](AdbError::IndexNotFound) for an unknown index and
    /// [`IndexArity`](AdbError::IndexArity) for more values than the index has columns.
    pub fn find<T: AData>(&self, index: &str, values: &[Scalar]) -> AdbResult<Vec<T>> {
        self.query(index).prefixed(values).run()
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

    /// Runs a built index query: a prefix scan (exact = full prefix), optional
    /// subtree scoping, optional reversal, then recomposes each hit as a `T`.
    pub(crate) fn execute_query<T: AData>(
        &self,
        index: &str,
        prefix: &[Scalar],
        reverse: bool,
        root: &APath,
    ) -> AdbResult<Vec<T>> {
        let entry = {
            let meta = self.txn.open_table(META_TABLE)?;

            registry::lookup(&meta, &self.table, index)?
        }
        .ok_or_else(|| AdbError::IndexNotFound {
            index: index.to_string(),
        })?;

        let def = entry.def();
        if prefix.len() > def.columns().len() {
            return Err(AdbError::IndexArity {
                index:    index.to_string(),
                expected: def.columns().len(),
                got:      prefix.len(),
            });
        }

        // `encode_columns` zips with the columns, so a short `prefix` encodes only
        // its leading columns — exactly the byte prefix a prefix scan needs.
        let cols = def.encode_columns(prefix);

        let index_table = match self.txn.open_table(INDEX_TABLE) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        let mut entities = scan::scan_prefix(&index_table, entry.id(), &cols, def.unique())?;

        let Some(data) = self.open()? else {
            return Ok(Vec::new());
        };

        // Restrict to entities at or under `root` (a rooted view scopes here).
        if !root.is_empty() {
            let pattern = Pattern::parse(def.pattern())?;
            if pattern.depth() < root.len() {
                return Ok(Vec::new());
            }

            let under: HashSet<AKey> = pattern.affected_entities(&data, root)?.into_iter().collect();
            entities.retain(|entity| under.contains(entity));
        }

        // The scan yields ascending index order; reverse the materialized hits for
        // descending order.
        if reverse {
            entities.reverse();
        }

        // Each match is addressed by its stable key; recompose it from its own blob.
        let mut out = Vec::with_capacity(entities.len());
        for entity in entities {
            if let Some(value) = self.load_entity::<T>(&data, entity)? {
                out.push(value);
            }
        }

        Ok(out)
    }

    /// Recomposes the file stored under `entity` as a `T`, or `None` when the
    /// entity is absent or is a directory (not a decodable value).
    fn load_entity<T: AData>(&self, table: &ReadOnlyTable<u128, EntryBytes>, entity: AKey) -> AdbResult<Option<T>> {
        // An index query silently skips entities the principal may not read.
        #[cfg(feature = "permissions")]
        match self.authorize_target(entity, Right::Read) {
            Ok(()) => {}
            Err(AdbError::PermissionDenied(_)) => return Ok(None),
            Err(err) => return Err(err),
        }

        let Some(blob) = self.entry_blob(table, entity)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_integrity(entity, &blob)?;

        match get_entry_kind(&blob)? {
            EntryKind::File => {
                let reader = ArchivedReader::new(blob, 1);

                Ok(Some(T::load(&reader, &VPath::root())?))
            }
            EntryKind::Dir => Ok(None),
        }
    }

    /// Lists the direct children of the directory at `path`, as `(name, kind)`
    /// pairs in name order. Errors if `path` names a file.
    pub fn ls(&self, path: impl IntoArborPath) -> AdbResult<Vec<Entry>> {
        let path = path.into_arbor_path()?;
        let Some(table) = self.open()? else {
            return Ok(Vec::new());
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Err(AdbError::ValueNotFound(path));
        };

        #[cfg(feature = "permissions")]
        self.authorize_target(akey, Right::Read)?;

        #[cfg(feature = "entry-timestamps")]
        self.record_access(akey);

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(Vec::new()); // the root directory, not yet materialized
        };

        #[cfg(feature = "permissions")]
        self.verify_integrity(akey, &blob)?;

        let (kind, payload) = entry_split(&blob)?;
        if kind != EntryKind::Dir {
            return Err(AdbError::CannotAccess(format!("'{path}' is a file, not a directory")));
        }

        let dir = ArchivedDir::new(payload)?;
        let mut out = Vec::with_capacity(dir.len()?);
        for (name, child) in dir.entries()? {
            let child_blob = self
                .entry_blob(&table, child)?
                .ok_or_else(|| AdbError::Corrupt("a directory entry points at a missing vnode".into()))?;

            out.push(Entry::new(name.to_string(), get_entry_kind(&child_blob)?));
        }

        Ok(out)
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
