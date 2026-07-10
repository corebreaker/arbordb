//! The opaque write transaction: serialized mutation of one table.

use super::{
    context::{table, Context},
    grab::{self, Grab},
    rooted::RootedWrite,
    IndexQuery,
};

use crate::{
    access::{MemWriter, MutCursor, Writer},
    codec::decode,
    data::{AData, AMut, ARef, AValue, Scalar},
    db::DbInner,
    engine::{data_def, entry_split, fetch_entry_kind, read_entry, resolve, Entry, EntryKind, INDEX_TABLE, META_TABLE},
    error::{AdbError, AdbResult},
    index::{
        maintenance,
        registry::{self, IndexEntry},
        scan,
        Pattern,
    },
    path::{APath, IntoArborPath, IntoValuePath, VPath},
    value::Value,
    AKey,
};

use redb::WriteTransaction;
use std::sync::Arc;

#[cfg(feature = "entry-timestamps")]
use crate::{
    inode::{self, read_timestamps, NodeTimestamps, INODES_TABLE},
    time::timestamp_now,
};

#[cfg(feature = "permissions")]
use crate::{
    acl::{AclClass, Rights},
    codec::ArchivedDir,
    engine::EntryBytes,
    inode::{read_acl, read_mac, Acl},
    perm::{self, Principal},
};

#[cfg(feature = "permissions")]
use redb::ReadableTable;

/// Deletes every value owned by `uid` in `table`, within `txn`, bypassing ACL
/// checks (the caller must already be an authorized administrator). Used by user
/// removal so that no value is left orphaned.
#[cfg(feature = "permissions")]
pub(crate) fn reap_owned_in(txn: &WriteTransaction, table: &str, uid: u32, principal: &Principal) -> AdbResult<()> {
    let mut data = txn.open_table(data_def(table))?;
    let mut inodes = txn.open_table(INODES_TABLE)?;
    let mut ctx = Context::new(&mut data, &mut inodes, table, principal);

    ctx.reap_owned(uid)
}

/// A write transaction over one table. Changes become durable on [`commit`](WriteTxn::commit).
pub struct WriteTxn {
    /// The underlying engine write transaction.
    txn:   WriteTransaction,
    /// The table this transaction writes.
    table: String,
    /// The database-wide shared state (for the generation bump on commit).
    inner: Arc<DbInner>,

    /// The identity performing the writes; drives ACL enforcement.
    #[cfg(feature = "permissions")]
    principal: Arc<Principal>,
}

impl WriteTxn {
    pub(crate) fn new(
        txn: WriteTransaction,
        table: String,
        inner: Arc<DbInner>,
        #[cfg(feature = "permissions")] principal: Arc<Principal>,
    ) -> Self {
        Self {
            txn,
            table,
            inner,
            #[cfg(feature = "permissions")]
            principal,
        }
    }

    /// Stores a typed value as a file at `path`, creating parent directories as
    /// needed and replacing whatever was there. The value is decomposed into an
    /// in-memory tree, then encoded to one blob.
    pub fn store<T: AData>(&self, path: impl IntoArborPath, value: &T) -> AdbResult<()> {
        let writer = MemWriter::new();
        value.store(&writer, &VPath::root())?;

        self.store_value(path, &writer.into_value())
    }

    /// Stores any [`serde::Serialize`] value as a file at `path`, encoding it
    /// **straight into the value blob** — no intermediate [`Value`] tree. It is stored
    /// natively (indexable and navigable by `VPath`, exactly like a value built through
    /// [`store`](Self::store)). Read it back with
    /// [`ReadTxn::load_serde_value`](crate::txn::ReadTxn::load_serde_value).
    #[cfg(feature = "serde")]
    pub fn store_serde_value<T: serde::Serialize + ?Sized>(
        &self,
        path: impl IntoArborPath,
        value: &T,
    ) -> AdbResult<()> {
        let path = path.into_arbor_path()?;
        let blob = crate::serde::to_blob(value)?;

        self.store_blob_at(&path, &blob)
    }

    /// Stores a dynamic [`Value`] as a file at `path`, creating parent directories
    /// as needed. Replaces whatever was there: a file overwrite keeps the vnode's
    /// identity; a directory is removed with its whole subtree first.
    pub fn store_value(&self, path: impl IntoArborPath, value: &Value) -> AdbResult<()> {
        self.store_value_at(&path.into_arbor_path()?, value)
    }

    /// Stores `value` as a file at an already-parsed access path.
    pub(crate) fn store_value_at(&self, path: &APath, value: &Value) -> AdbResult<()> {
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot store a file at the root")));
        }

        self.reindex_around(std::slice::from_ref(path), |ctx| {
            table::store_value_into(ctx, path, value)
        })
    }

    /// Stores an already-encoded value blob at an already-parsed access path — the
    /// direct Serde write path, which skips the intermediate [`Value`].
    #[cfg(feature = "serde")]
    pub(crate) fn store_blob_at(&self, path: &APath, value_blob: &[u8]) -> AdbResult<()> {
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot store a file at the root")));
        }

        self.reindex_around(std::slice::from_ref(path), |ctx| {
            table::store_blob_into(ctx, path, value_blob)
        })
    }

    /// Sets the scalar at `at` inside the file at `path`. When the new scalar encodes
    /// to the same width as the one already there, the blob is patched in place — no
    /// decode, no re-encode — otherwise the value is decoded, updated, and re-encoded.
    /// Registered indexes are maintained across either path.
    pub(crate) fn put_scalar_at(&self, path: &APath, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        self.reindex_around(std::slice::from_ref(path), |ctx| {
            table::put_scalar_into(ctx, path, at, &scalar)
        })
    }

    /// Opens a mutable accessor over the file at `path`, or `None` if absent. A scalar
    /// overwrite through the accessor that keeps the leaf's byte width patches the blob
    /// in place (no decode, no re-encode); a structural change still rewrites it. The
    /// accessor borrows the transaction, so drop it before `commit`.
    pub fn fetch_mut<'t, A: AMut<'t>>(&'t self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        let apath = path.into_arbor_path()?;

        // Presence check without decoding the value — resolve the vnode and read its
        // kind alone.
        {
            let table = self.txn.open_table(data_def(&self.table))?;
            match resolve(&table, &apath)? {
                Some(akey) => match fetch_entry_kind(&table, akey)? {
                    Some(EntryKind::File) => {}
                    Some(EntryKind::Dir) => {
                        return Err(AdbError::CannotAccess(format!("'{apath}' is a directory, not a file")));
                    }
                    None => return Ok(None),
                },
                None => return Ok(None),
            }
        }

        let cursor: Arc<dyn Writer + 't> = Arc::new(MutCursor::open(self, apath));

        Ok(Some(A::open(cursor, VPath::root())))
    }

    /// Verifies vnode `akey`'s integrity tag over `entry` and its current ACL, so a
    /// writer's own read-back (through [`load_value_at`](Self::load_value_at) and the
    /// mutable accessor) detects a blob altered outside the library rather than
    /// silently returning — or re-sealing over — it. A no-op unless this handle is an
    /// authenticated user holding the integrity key.
    #[cfg(feature = "permissions")]
    fn verify_entry(&self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        if !matches!(self.principal.as_ref(), Principal::User(_)) {
            return Ok(());
        }

        let inodes = self.txn.open_table(INODES_TABLE)?;

        self.verify_with(&inodes, akey, entry)
    }

    /// Verifies `akey`'s keyed MAC over `entry` and its ACL, using an already-open
    /// `$inodes` handle so a caller mid-walk need not reopen the table (a write
    /// transaction refuses a second open of the same table). A no-op unless this
    /// handle is an authenticated user — the only writer that holds the key.
    #[cfg(feature = "permissions")]
    fn verify_with<R>(&self, inodes: &R, akey: AKey, entry: &[u8]) -> AdbResult<()>
    where
        R: ReadableTable<&'static [u8], &'static [u8]>, {
        let Principal::User(session) = self.principal.as_ref() else {
            return Ok(());
        };

        let acl = read_acl(inodes, &self.table, akey)?
            .map(|acl| acl.encode())
            .unwrap_or_default();
        let stored = read_mac(inodes, &self.table, akey)?;
        let expected = perm::integrity::mac_value(session.key(), &self.table, akey, entry, &acl);

        match stored {
            Some(mac) if perm::integrity::ct_eq(&mac, &expected) => Ok(()),
            _ => Err(AdbError::Tampered(format!(
                "integrity check failed for a vnode in table '{}'",
                self.table
            ))),
        }
    }

    /// Whether ACL enforcement applies — a protected database written as an
    /// authenticated user, not an unrestricted handle. (A guest cannot write.)
    #[cfg(feature = "permissions")]
    fn enforced(&self) -> bool {
        !matches!(self.principal.as_ref(), Principal::Unrestricted)
    }

    /// Resolves `path` over the write transaction's own tables with an `Access`
    /// check (and integrity verify) on every directory traversed. `None` if a
    /// component along the way is missing.
    #[cfg(feature = "permissions")]
    fn resolve_enforced<R>(&self, data: &R, path: &APath) -> AdbResult<Option<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let inodes = self.txn.open_table(INODES_TABLE)?;

        let mut akey = AKey::ROOT;
        for name in path.names() {
            let acl = read_acl(&inodes, &self.table, akey)?;
            perm::access::authorize(&self.principal, akey, acl.as_ref(), Rights::Access)?;

            let Some(entry) = read_entry(data, akey)? else {
                return Ok(None);
            };

            // A tampered directory blob could redirect a name to another vnode, so
            // verify each directory descended through.
            self.verify_with(&inodes, akey, &entry)?;

            let (kind, payload) = entry_split(&entry)?;
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

    /// Loads the current (uncommitted) value of the file at `path`, or `None` if
    /// absent. Errors if `path` names a directory.
    pub(crate) fn load_value_at(&self, path: &APath) -> AdbResult<Option<Value>> {
        let table = self.txn.open_table(data_def(&self.table))?;

        let Some(akey) = resolve(&table, path)? else {
            return Ok(None);
        };

        let Some(entry) = read_entry(&table, akey)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_entry(akey, &entry)?;

        let (kind, payload) = entry_split(&entry)?;
        match kind {
            EntryKind::File => Ok(Some(decode(payload)?)),
            EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
        }
    }

    /// Creates the directory at `path` (and any missing ancestors). Idempotent;
    /// errors if a path component is an existing file.
    pub fn mkdir(&self, path: impl IntoArborPath) -> AdbResult<()> {
        let path = path.into_arbor_path()?;
        let mut data = self.txn.open_table(data_def(&self.table))?;

        #[cfg(feature = "entry-timestamps")]
        let mut inodes = self.txn.open_table(INODES_TABLE)?;

        #[cfg(feature = "permissions")]
        let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());
        #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
        let mut ctx = Context::new(&mut data, &mut inodes, &self.table);
        #[cfg(not(feature = "entry-timestamps"))]
        let mut ctx = Context::new(&mut data);

        table::ensure_dir(&mut ctx, &path)?;

        Ok(())
    }

    /// Changes the owner of the file or directory at `path` to the user named
    /// `owner`. Only the master user, a master-group member, or the current owner
    /// may do this (write access alone is not enough).
    #[cfg(feature = "permissions")]
    pub fn chown(&self, path: impl IntoArborPath, owner: &str) -> AdbResult<()> {
        let path = path.into_arbor_path()?;

        let new_uid = {
            let meta = self.txn.open_table(META_TABLE)?;
            perm::store::uid_of(&meta, owner)?
        }
        .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{owner}'")))?;

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::chown_into(&mut ctx, &path, new_uid)
    }

    /// Resolves a group name to its id, erroring if no such group exists.
    #[cfg(feature = "permissions")]
    fn require_gid(&self, group: &str) -> AdbResult<u32> {
        let meta = self.txn.open_table(META_TABLE)?;

        perm::store::gid_of(&meta, group)?.ok_or_else(|| AdbError::CannotAccess(format!("no group named '{group}'")))
    }

    /// Grants `rights` to `class` on the file or directory at `path`. For a
    /// [`Group`](AclClass::Group) this inserts or updates that group's entry — adding
    /// the vnode to the group if needed — and setting a group to [`Rights::None`]
    /// removes it. Requires `Modify` on the vnode; the root's ACL cannot be changed.
    #[cfg(feature = "permissions")]
    pub fn set_acl(&self, path: impl IntoArborPath, class: AclClass, rights: Rights) -> AdbResult<()> {
        let path = path.into_arbor_path()?;

        // Resolve a group name to its id before opening the write context.
        let gid = match &class {
            AclClass::Group(name) => Some(self.require_gid(name)?),
            _ => None,
        };

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::set_acl_into(&mut ctx, &path, |acl| match class {
            AclClass::User => acl.set_owner_rights(rights),
            AclClass::Other => acl.set_other_rights(rights),
            AclClass::Group(_) => acl.set_group_rights(gid.expect("a group class resolves a gid"), rights),
        })
    }

    /// Adds the file or directory at `path` to `group`, granting that group
    /// [`Access`](Rights::Access) unless it already has an entry (whose grade is
    /// then kept). Use [`set_acl`](Self::set_acl) to grant a stronger grade.
    /// Requires `Modify` on the vnode.
    #[cfg(feature = "permissions")]
    pub fn add_group(&self, path: impl IntoArborPath, group: &str) -> AdbResult<()> {
        let path = path.into_arbor_path()?;
        let gid = self.require_gid(group)?;

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::set_acl_into(&mut ctx, &path, |acl| acl.add_group(gid))
    }

    /// Removes the file or directory at `path` from `group` (a no-op if it is not a
    /// member). Requires `Modify` on the vnode.
    #[cfg(feature = "permissions")]
    pub fn del_group(&self, path: impl IntoArborPath, group: &str) -> AdbResult<()> {
        let path = path.into_arbor_path()?;
        let gid = self.require_gid(group)?;

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::set_acl_into(&mut ctx, &path, |acl| acl.remove_group(gid))
    }

    /// Removes the file or directory at `path` (a directory with its whole
    /// subtree). Returns whether anything was removed.
    pub fn rm(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        let path = path.into_arbor_path()?;
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot remove the root")));
        }

        self.reindex_around(std::slice::from_ref(&path), |ctx| table::rm_into(ctx, &path))
    }

    /// Moves the vnode at `src` to `dst`, keeping its identity (a relink, not a
    /// copy — so `mv` is O(1) and any accessor holding the vnode's `AKey` stays
    /// valid). Creates `dst`'s parent directories and replaces an existing `dst`.
    /// Errors if `dst` is `src` itself or a descendant of it.
    pub fn mv(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        let src = src.into_arbor_path()?;
        let dst = dst.into_arbor_path()?;

        if dst.names().starts_with(src.names()) {
            return Err(AdbError::CannotAccess(String::from(
                "cannot move a vnode onto itself or into a descendant",
            )));
        }

        if src.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot move the root")));
        }
        if dst.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot move onto the root")));
        }

        // The entity leaves `src` and appears at `dst`, so both scopes are re-indexed.
        let scopes = [src.clone(), dst.clone()];

        self.reindex_around(&scopes, |ctx| table::mv_into(ctx, &src, &dst))
    }

    /// Copies the subtree at `src` to `dst` under fresh identities (a deep copy).
    /// Creates `dst`'s parent directories and replaces an existing `dst`. Errors
    /// if `dst` is `src` itself or a descendant of it.
    pub fn cp(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        let src = src.into_arbor_path()?;
        let dst = dst.into_arbor_path()?;

        if dst.names().starts_with(src.names()) {
            return Err(AdbError::CannotAccess(String::from(
                "cannot copy a vnode onto itself or into a descendant",
            )));
        }

        if dst.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot copy onto the root")));
        }

        // A copy creates fresh entities at `dst`; `src` is unchanged, so only `dst`
        // needs re-indexing.
        self.reindex_around(std::slice::from_ref(&dst), |ctx| table::cp_into(ctx, &src, &dst))
    }

    /// Loads the table's registered indexes (empty when it has none).
    fn indexes(&self) -> AdbResult<Vec<IndexEntry>> {
        let meta = self.txn.open_table(META_TABLE)?;

        registry::for_table(&meta, &self.table)
    }

    /// Brackets a mutation with index maintenance: remove the affected entities'
    /// current index entries, apply the mutation, then insert the entries the new
    /// state implies. An unindexed table takes the zero-overhead path.
    fn reindex_around<T>(
        &self,
        scopes: &[APath],
        apply: impl FnOnce(&mut Context<'_, '_>) -> AdbResult<T>,
    ) -> AdbResult<T> {
        let indexes = self.indexes()?;
        let mut data = self.txn.open_table(data_def(&self.table))?;

        #[cfg(feature = "entry-timestamps")]
        let mut inodes = self.txn.open_table(INODES_TABLE)?;

        if indexes.is_empty() {
            #[cfg(feature = "permissions")]
            let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());
            #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
            let mut ctx = Context::new(&mut data, &mut inodes, &self.table);
            #[cfg(not(feature = "entry-timestamps"))]
            let mut ctx = Context::new(&mut data);

            return apply(&mut ctx);
        }

        let mut index = self.txn.open_table(INDEX_TABLE)?;

        for scope in scopes {
            maintenance::delete(&data, &mut index, &indexes, scope)?;
        }

        // Build the write context only around the mutation itself, so index
        // maintenance keeps its shared borrow of the data table before and after.
        let result = {
            #[cfg(feature = "permissions")]
            let mut ctx = Context::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());
            #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
            let mut ctx = Context::new(&mut data, &mut inodes, &self.table);
            #[cfg(not(feature = "entry-timestamps"))]
            let mut ctx = Context::new(&mut data);

            apply(&mut ctx)?
        };

        for scope in scopes {
            maintenance::insert(&data, &mut index, &indexes, scope)?;
        }

        Ok(result)
    }

    /// A view of this transaction whose access paths are relative to `root`.
    pub fn rooted(&self, root: impl IntoArborPath) -> AdbResult<RootedWrite<'_>> {
        Ok(RootedWrite::new(self, root.into_arbor_path()?))
    }

    // -- Reads over this transaction's own (uncommitted) state ---------------
    //
    // A writer reads exactly what a reader does, but over its own pending
    // changes and without the caches — so a read-modify-write stays atomic
    // within one transaction. See the [`ReadOps`] surface for the shared bodies.

    /// Loads a typed value from the file at `path`, or `None` if absent. Errors if
    /// `path` names a directory.
    pub fn load<T: AData>(&self, path: impl IntoArborPath) -> AdbResult<Option<T>> {
        grab::load(self, path)
    }

    /// Loads a file stored at `path` into any [`serde::de::DeserializeOwned`] value,
    /// reading the value blob directly over the zero-copy codec. `None` if absent;
    /// errors if `path` names a directory.
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
    /// For a *mutable* accessor use [`fetch_mut`](Self::fetch_mut).
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
    /// `None` if the vnode is absent or has no recorded metadata yet.
    #[cfg(feature = "entry-timestamps")]
    pub fn times(&self, path: impl IntoArborPath) -> AdbResult<Option<NodeTimestamps>> {
        grab::times(self, path)
    }

    /// The [`Rights`] the file or directory at `path` grants `class`. Errors with
    /// [`ValueNotFound`](AdbError::ValueNotFound) if nothing exists at `path`.
    #[cfg(feature = "permissions")]
    pub fn get_acl(&self, path: impl IntoArborPath, class: AclClass) -> AdbResult<Rights> {
        grab::get_acl(self, path, class)
    }

    /// The name of the owner of the file or directory at `path`, or `None` if the
    /// vnode is absent or has no ACL.
    #[cfg(feature = "permissions")]
    pub fn owner(&self, path: impl IntoArborPath) -> AdbResult<Option<String>> {
        grab::owner(self, path)
    }

    /// The names of the groups the file or directory at `path` belongs to, sorted.
    #[cfg(feature = "permissions")]
    pub fn groups(&self, path: impl IntoArborPath) -> AdbResult<Vec<String>> {
        grab::groups(self, path)
    }

    /// Finds the entities an index points at, recomposing each as a `T`. Sees this
    /// transaction's own uncommitted changes.
    pub fn find<T: AData>(&self, index: &str, values: &[Scalar]) -> AdbResult<Vec<T>> {
        grab::find(self, index, values)
    }

    /// Starts an [`IndexQuery`] against `index` — a builder for prefix matches,
    /// reverse order, and subtree scoping. Sees this transaction's own uncommitted
    /// changes.
    pub fn query(&self, index: &str) -> IndexQuery<'_> {
        IndexQuery::new(self, index)
    }

    /// Commits the transaction, making its changes durable and advancing the
    /// database generation (so cached resolutions from earlier snapshots retire).
    pub fn commit(self) -> AdbResult<()> {
        let WriteTxn {
            txn,
            inner,
            ..
        } = self;

        // Persist any buffered read access times in this same transaction.
        #[cfg(feature = "entry-timestamps")]
        {
            let batch = inner.drain_access_log();
            if !batch.is_empty() {
                let mut inodes = txn.open_table(INODES_TABLE)?;
                for ((table, akey), when) in batch {
                    inode::bump_access(&mut inodes, &table, akey, when)?;
                }
            }
        }

        let guard = inner
            .version_lock()
            .write()
            .map_err(|_| AdbError::CannotAccess(String::from("the version lock was poisoned")))?;

        txn.commit()?;
        inner.bump_generation();
        drop(guard);

        Ok(())
    }
}

impl Grab for WriteTxn {
    fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
        let data = self.txn.open_table(data_def(&self.table))?;

        #[cfg(feature = "permissions")]
        if self.enforced() {
            return self.resolve_enforced(&data, path);
        }

        crate::engine::resolve(&data, path)
    }

    fn entry_blob(&self, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>> {
        let data = self.txn.open_table(data_def(&self.table))?;

        Ok(read_entry(&data, akey)?.map(Arc::new))
    }

    fn lookup_index(&self, index: &str) -> AdbResult<Option<IndexEntry>> {
        let meta = self.txn.open_table(META_TABLE)?;

        registry::lookup(&meta, &self.table, index)
    }

    fn scan_index(&self, entry: &IndexEntry, cols: &[u8]) -> AdbResult<Vec<AKey>> {
        let index_table = self.txn.open_table(INDEX_TABLE)?;

        scan::scan_prefix(&index_table, entry.id(), cols, entry.def().unique())
    }

    fn affected_under(&self, pattern: &Pattern, root: &APath) -> AdbResult<Vec<AKey>> {
        let data = self.txn.open_table(data_def(&self.table))?;

        pattern.affected_entities(&data, root)
    }

    #[cfg(feature = "permissions")]
    fn authorize(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        if !self.enforced() {
            return Ok(());
        }

        let inodes = self.txn.open_table(INODES_TABLE)?;
        let acl = read_acl(&inodes, &self.table, akey)?;

        perm::access::authorize(&self.principal, akey, acl.as_ref(), needed)
    }

    #[cfg(feature = "permissions")]
    fn verify(&self, akey: AKey, blob: &[u8]) -> AdbResult<()> {
        self.verify_entry(akey, blob)
    }

    #[cfg(feature = "permissions")]
    fn acl_of(&self, akey: AKey) -> AdbResult<Option<Acl>> {
        let inodes = self.txn.open_table(INODES_TABLE)?;

        read_acl(&inodes, &self.table, akey)
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
        // A writer deposits directly into the database-wide log; `commit` persists
        // it in this same transaction.
        self.inner.deposit_access(&self.table, vec![(akey, timestamp_now())]);
    }

    #[cfg(feature = "entry-timestamps")]
    fn timestamps_of(&self, akey: AKey) -> AdbResult<Option<NodeTimestamps>> {
        let inodes = self.txn.open_table(INODES_TABLE)?;

        read_timestamps(&inodes, &self.table, akey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codec::encode,
        engine::file_entry,
        index::{registry, IndexColumn, IndexDef},
        data::Scalar,
        ArborDb,
    };

    use redb::ReadableTable; // `index.iter()` in the assertions below
    use std::collections::BTreeMap;

    fn user(age: i64) -> Value {
        Value::Node(BTreeMap::from([(String::from("age"), Value::Leaf(Scalar::I64(age)))]))
    }

    /// The number of physical entries in the shared index table.
    fn index_entries(w: &WriteTxn) -> usize {
        let index = w.txn.open_table(INDEX_TABLE).unwrap();

        index.iter().unwrap().count()
    }

    #[test]
    fn store_and_rm_maintain_a_registered_index() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();
        let w = table.write().unwrap();

        // Register a `users/*` index on the `age` column (raw, via the txn — the
        // public `create_index` lands in a later sub-phase).
        {
            let mut meta = w.txn.open_table(META_TABLE).unwrap();
            let def = IndexDef::new(
                String::from("by_age"),
                String::from("users/*"),
                vec![IndexColumn::asc(VPath::root().child_name("age"))],
                false,
            );

            registry::create(&mut meta, "t", &def).unwrap();
        }

        // Each store under the pattern adds one entry.
        w.store_value("users/alice", &user(30)).unwrap();
        w.store_value("users/bob", &user(40)).unwrap();
        assert_eq!(index_entries(&w), 2);

        // Editing a column rewrites the entity's entry — still one per entity.
        w.store_value("users/alice", &user(31)).unwrap();
        assert_eq!(index_entries(&w), 2);

        // A store outside the pattern is not indexed.
        w.store_value("orgs/acme", &user(99)).unwrap();
        assert_eq!(index_entries(&w), 2);

        // Removing an entity drops its entry.
        w.rm("users/bob").unwrap();
        assert_eq!(index_entries(&w), 1);

        w.commit().unwrap();
    }

    #[test]
    fn patch_scalar_replaces_a_same_width_leaf_in_place() {
        let mut entry = file_entry(&encode(&user(30)));
        let len_before = entry.len();
        let age = VPath::root().child_name("age");

        // I64 → I64 keeps the width, so the blob is patched without changing length.
        assert!(table::patch_scalar(&mut entry, &age, &Scalar::I64(31)).unwrap());
        assert_eq!(entry.len(), len_before);

        let (kind, payload) = entry_split(&entry).unwrap();
        assert_eq!(kind, EntryKind::File);
        assert_eq!(decode(payload).unwrap(), user(31));
    }

    #[test]
    fn patch_scalar_declines_a_width_change_or_non_leaf() {
        let mut entry = file_entry(&encode(&user(30)));

        // A width change (I64 → Str), a non-leaf path (the object root), and an absent
        // field all decline, leaving the caller to re-encode.
        let age = VPath::root().child_name("age");
        assert!(!table::patch_scalar(&mut entry, &age, &Scalar::Str(String::from("thirty"))).unwrap());
        assert!(!table::patch_scalar(&mut entry, &VPath::root(), &Scalar::I64(1)).unwrap());

        let missing = VPath::root().child_name("missing");
        assert!(!table::patch_scalar(&mut entry, &missing, &Scalar::I64(1)).unwrap());

        // A declined patch leaves the entry byte-for-byte unchanged.
        let (_, payload) = entry_split(&entry).unwrap();
        assert_eq!(decode(payload).unwrap(), user(30));
    }

    #[test]
    fn an_in_place_scalar_edit_maintains_a_registered_index() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        {
            let w = table.write().unwrap();

            {
                let mut meta = w.txn.open_table(META_TABLE).unwrap();
                let def = IndexDef::new(
                    String::from("by_age"),
                    String::from("users/*"),
                    vec![IndexColumn::asc(VPath::root().child_name("age"))],
                    false,
                );

                registry::create(&mut meta, "t", &def).unwrap();
            }

            w.store_value("users/alice", &user(30)).unwrap();
            assert_eq!(index_entries(&w), 1);

            // An in-place edit of the indexed column keeps exactly one entry: the old
            // key is removed and the new one inserted around the patch.
            let age = VPath::root().child_name("age");
            w.put_scalar_at(&APath::parse("users/alice").unwrap(), &age, Scalar::I64(31))
                .unwrap();
            assert_eq!(index_entries(&w), 1);

            w.commit().unwrap();
        }

        // The edit persisted.
        let r = table.read().unwrap();
        assert_eq!(r.get_as::<i64>("users/alice", "age").unwrap(), Some(31));
    }
}
