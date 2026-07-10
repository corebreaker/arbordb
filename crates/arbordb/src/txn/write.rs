//! The opaque write transaction: serialized mutation of one table.

use super::{
    context::{table, Context},
    rooted::RootedWrite,
};

use crate::{
    access::{MemWriter, MutCursor, Writer},
    codec::decode,
    data::{AData, AMut, Scalar},
    db::DbInner,
    engine::{data_def, fetch_entry_kind, read_entry, resolve, entry_split, EntryKind, INDEX_TABLE, META_TABLE},
    error::{AdbError, AdbResult},
    index::{
        maintenance,
        registry::{self, IndexEntry},
    },
    path::{APath, IntoArborPath, VPath},
    value::Value,
};

use redb::WriteTransaction;
use std::sync::Arc;

#[cfg(feature = "entry-timestamps")]
use crate::inode::{self, INODES_TABLE};

#[cfg(feature = "permissions")]
use crate::{
    acl::{AclClass, Rights},
    inode::{read_acl, read_mac},
    perm::{self, Principal},
    AKey,
};

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
        let Principal::User(session) = self.principal.as_ref() else {
            return Ok(());
        };

        let inodes = self.txn.open_table(INODES_TABLE)?;
        let acl = read_acl(&inodes, &self.table, akey)?
            .map(|acl| acl.encode())
            .unwrap_or_default();
        let stored = read_mac(&inodes, &self.table, akey)?;
        let expected = perm::integrity::mac_value(session.key(), &self.table, akey, entry, &acl);

        match stored {
            Some(mac) if perm::integrity::ct_eq(&mac, &expected) => Ok(()),
            _ => Err(AdbError::Tampered(format!(
                "integrity check failed for a vnode in table '{}'",
                self.table
            ))),
        }
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
    fn gid_of(&self, group: &str) -> AdbResult<u32> {
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
            AclClass::Group(name) => Some(self.gid_of(name)?),
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
        let gid = self.gid_of(group)?;

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
        let gid = self.gid_of(group)?;

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
