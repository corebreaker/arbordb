//! The opaque write transaction: serialized mutation of one table.

use super::rooted::RootedWrite;
use crate::{
    access::{MemWriter, MutCursor, Writer},
    codec::{decode, encode, encode_dir, ArchivedDir, ArchivedValue},
    data::{AData, AMut, Scalar},
    db::DbInner,
    engine::{
        child_of,
        data_def,
        dir_entry,
        fetch_entry_kind,
        file_entry,
        read_entry,
        resolve,
        entry_split,
        EntryBytes,
        EntryKind,
        INDEX_TABLE,
        META_TABLE,
    },
    error::{AdbError, AdbResult},
    index::{
        maintenance,
        registry::{self, IndexEntry},
    },
    path::{APath, IntoArborPath, VPath},
    value::Value,
    AKey,
};

use redb::{ReadableTable, WriteTransaction};
use std::{collections::BTreeMap, sync::Arc};

/// The per-transaction data table (borrows the write transaction).
type DataTable<'txn> = redb::Table<'txn, u128, EntryBytes>;

/// The virtual-filesystem write helpers backing the public [`WriteTxn`] ops.
///
/// These routines operate directly on the raw [`DataTable`] — resolving,
/// linking, rewriting, and cascading over directory and file nodes — rather
/// than on `&self`. Grouping them here keeps them off [`WriteTxn`]'s surface
/// while letting each public op compose them.
mod table {
    use super::*;

    /// Stores `value` as a file at `path` (a non-root path), creating parents and
    /// replacing whatever was there.
    pub(super) fn store_value_into(table: &mut DataTable, path: &APath, value: &Value) -> AdbResult<()> {
        let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");
        let parent = ensure_dir(table, &parent_path)?;

        let akey = match child_of(&*table, parent, name)? {
            Some(node) => {
                if fetch_entry_kind(&*table, node)? == Some(EntryKind::File) {
                    node
                } else {
                    cascade_delete(table, node)?;
                    let fresh = AKey::generate();
                    link_child(table, parent, name, fresh)?;
                    fresh
                }
            }
            None => {
                let fresh = AKey::generate();
                link_child(table, parent, name, fresh)?;
                fresh
            }
        };

        let entry = file_entry(&encode(value));
        table.insert(u128::from(akey), entry.as_slice())?;

        Ok(())
    }

    /// Sets the scalar at `at` inside the file at `path`. When the new scalar keeps the
    /// current leaf's byte width the blob is patched in place — no decode, no
    /// re-encode; otherwise the value is decoded, updated, and re-encoded. Creates the
    /// file (and its parents) when it does not exist yet.
    pub(super) fn put_scalar_into(table: &mut DataTable, path: &APath, at: &VPath, scalar: &Scalar) -> AdbResult<()> {
        let akey = resolve(&*table, path)?;
        let entry = match akey {
            Some(akey) => read_entry(&*table, akey)?,
            None => None,
        };

        let mut value = match (akey, entry) {
            (Some(akey), Some(mut entry)) => {
                // Fast path: a leaf that keeps its width is patched in place — every
                // other offset in the blob stays valid, so nothing is re-encoded.
                if patch_scalar(&mut entry, at, scalar)? {
                    table.insert(u128::from(akey), entry.as_slice())?;

                    return Ok(());
                }

                // Slow path: decode the (untouched) blob to re-encode it below.
                let (kind, payload) = entry_split(&entry)?;
                match kind {
                    EntryKind::File => decode(payload)?,
                    EntryKind::Dir => {
                        return Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file")));
                    }
                }
            }
            _ => Value::default(),
        };

        value.set_value(at, Value::Leaf(scalar.clone()));

        store_value_into(table, path, &value)
    }

    /// Patches `entry` in place when its file's leaf at `at` keeps its encoded width
    /// under `scalar`, overwriting just that leaf's bytes and returning `true`. Returns
    /// `false`, leaving `entry` untouched, when the fast path does not apply (a
    /// directory, an absent or non-leaf path, or a width change) — the caller then
    /// re-encodes.
    pub(super) fn patch_scalar(entry: &mut [u8], at: &VPath, scalar: &Scalar) -> AdbResult<bool> {
        let mut encoded = Vec::new();
        scalar.encode(&mut encoded);

        // Locate the leaf's bytes, then drop the borrow before overwriting them.
        let start = {
            let (kind, payload) = entry_split(entry)?;
            if kind != EntryKind::File {
                return Ok(false);
            }

            let Some((scalar_off, current_len)) = ArchivedValue::new(payload)?.leaf_scalar_span(at)? else {
                return Ok(false);
            };

            if encoded.len() != current_len {
                return Ok(false);
            }

            // `scalar_off` is relative to the payload; the entry prefixes it with a
            // one-byte kind tag, so shift past that header.
            (entry.len() - payload.len()) + scalar_off
        };

        entry[start..start + encoded.len()].copy_from_slice(&encoded);

        Ok(true)
    }

    /// Removes the node at `path` (a non-root path) and its subtree. Returns whether
    /// anything was removed.
    pub(super) fn rm_into(table: &mut DataTable, path: &APath) -> AdbResult<bool> {
        let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");

        let Some(parent) = resolve(&*table, &parent_path)? else {
            return Ok(false);
        };
        let Some(akey) = child_of(&*table, parent, name)? else {
            return Ok(false);
        };

        cascade_delete(table, akey)?;
        unlink_child(table, parent, name)?;

        Ok(true)
    }

    /// Relinks the node at `src` to `dst`, keeping its identity.
    pub(super) fn mv_into(table: &mut DataTable, src: &APath, dst: &APath) -> AdbResult<()> {
        let (src_parent_path, src_name) = src.split_last().expect("a non-root path has a parent and a name");
        let (dst_parent_path, dst_name) = dst.split_last().expect("a non-root path has a parent and a name");

        let Some(src_parent) = resolve(&*table, &src_parent_path)? else {
            return Err(AdbError::ValueNotFound(src.clone()));
        };
        let Some(akey) = child_of(&*table, src_parent, src_name)? else {
            return Err(AdbError::ValueNotFound(src.clone()));
        };

        let dst_parent = ensure_dir(table, &dst_parent_path)?;
        if let Some(existing) = child_of(&*table, dst_parent, dst_name)? {
            cascade_delete(table, existing)?;
        }

        link_child(table, dst_parent, dst_name, akey)?;
        unlink_child(table, src_parent, src_name)?;

        Ok(())
    }

    /// Deep-copies the subtree at `src` to `dst` under fresh identities.
    pub(super) fn cp_into(table: &mut DataTable, src: &APath, dst: &APath) -> AdbResult<()> {
        let (dst_parent_path, dst_name) = dst.split_last().expect("a non-root path has a parent and a name");

        let Some(src_akey) = resolve(&*table, src)? else {
            return Err(AdbError::ValueNotFound(src.clone()));
        };

        let dst_parent = ensure_dir(table, &dst_parent_path)?;
        if let Some(existing) = child_of(&*table, dst_parent, dst_name)? {
            cascade_delete(table, existing)?;
        }

        let copy = deep_copy(table, src_akey)?;
        link_child(table, dst_parent, dst_name, copy)?;

        Ok(())
    }

    /// Reads a directory's children into an owned map (empty if the node is absent).
    pub(super) fn read_dir_map<R>(table: &R, akey: AKey) -> AdbResult<BTreeMap<String, AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let Some(entry) = read_entry(table, akey)? else {
            return Ok(BTreeMap::new());
        };

        let (kind, payload) = entry_split(&entry)?;
        if kind != EntryKind::Dir {
            return Err(AdbError::CannotAccess(String::from(
                "a path component is a file, not a directory",
            )));
        }

        let children = ArchivedDir::new(payload)?
            .entries()?
            .into_iter()
            .map(|(name, child)| (name.to_string(), child))
            .collect();

        Ok(children)
    }

    /// Writes `map` as directory `akey`'s children.
    pub(super) fn put_dir(table: &mut DataTable, akey: AKey, map: &BTreeMap<String, AKey>) -> AdbResult<()> {
        let entry = dir_entry(&encode_dir(map));
        table.insert(u128::from(akey), entry.as_slice())?;

        Ok(())
    }

    /// Adds (or replaces) a `name → child` link in directory `parent`.
    pub(super) fn link_child(table: &mut DataTable, parent: AKey, name: &str, child: AKey) -> AdbResult<()> {
        let mut map = read_dir_map(&*table, parent)?;
        map.insert(name.to_string(), child);

        put_dir(table, parent, &map)
    }

    /// Removes the `name` link from directory `parent`.
    pub(super) fn unlink_child(table: &mut DataTable, parent: AKey, name: &str) -> AdbResult<()> {
        let mut map = read_dir_map(&*table, parent)?;
        map.remove(name);

        put_dir(table, parent, &map)
    }

    /// Ensures the root directory exists.
    pub(super) fn ensure_root(table: &mut DataTable) -> AdbResult<()> {
        if read_entry(&*table, AKey::ROOT)?.is_none() {
            put_dir(table, AKey::ROOT, &BTreeMap::new())?;
        }

        Ok(())
    }

    /// Ensures every directory along `path` exists, returning the deepest one's key.
    pub(super) fn ensure_dir(table: &mut DataTable, path: &APath) -> AdbResult<AKey> {
        ensure_root(table)?;

        let mut akey = AKey::ROOT;
        for name in path.names() {
            match child_of(&*table, akey, name.as_str())? {
                Some(child) => {
                    if fetch_entry_kind(&*table, child)? != Some(EntryKind::Dir) {
                        return Err(AdbError::CannotAccess(format!("'{name}' is a file, not a directory")));
                    }

                    akey = child;
                }
                None => {
                    let child = AKey::generate();
                    put_dir(table, child, &BTreeMap::new())?;
                    link_child(table, akey, name.as_str(), child)?;
                    akey = child;
                }
            }
        }

        Ok(akey)
    }

    /// Removes node `akey` and, if it is a directory, its whole subtree.
    pub(super) fn cascade_delete(table: &mut DataTable, akey: AKey) -> AdbResult<()> {
        let children = match read_entry(&*table, akey)? {
            Some(entry) => {
                let (kind, payload) = entry_split(&entry)?;
                match kind {
                    EntryKind::Dir => ArchivedDir::new(payload)?
                        .entries()?
                        .into_iter()
                        .map(|(_, child)| child)
                        .collect::<Vec<_>>(),
                    EntryKind::File => Vec::new(),
                }
            }
            None => return Ok(()),
        };

        for child in children {
            cascade_delete(table, child)?;
        }

        table.remove(u128::from(akey))?;

        Ok(())
    }

    /// Deep-copies node `akey` (a file's blob verbatim, a directory recursively) under
    /// a freshly generated key, returning that key.
    pub(super) fn deep_copy(table: &mut DataTable, akey: AKey) -> AdbResult<AKey> {
        let entry = read_entry(&*table, akey)?.ok_or_else(|| AdbError::Corrupt("copying a missing node".into()))?;
        let (kind, payload) = entry_split(&entry)?;
        let fresh = AKey::generate();

        match kind {
            EntryKind::File => {
                table.insert(u128::from(fresh), entry.as_slice())?;
            }
            EntryKind::Dir => {
                let children: Vec<(String, AKey)> = ArchivedDir::new(payload)?
                    .entries()?
                    .into_iter()
                    .map(|(name, child)| (name.to_string(), child))
                    .collect();

                let mut copied = BTreeMap::new();
                for (name, child) in children {
                    copied.insert(name, deep_copy(table, child)?);
                }

                put_dir(table, fresh, &copied)?;
            }
        }

        Ok(fresh)
    }
}

/// A write transaction over one table. Changes become durable on [`commit`](WriteTxn::commit).
pub struct WriteTxn {
    txn:   WriteTransaction,
    table: String,
    inner: Arc<DbInner>,
}

impl WriteTxn {
    pub(crate) fn new(txn: WriteTransaction, table: String, inner: Arc<DbInner>) -> Self {
        Self {
            txn,
            table,
            inner,
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

    /// Stores a dynamic [`Value`] as a file at `path`, creating parent directories
    /// as needed. Replaces whatever was there: a file overwrite keeps the node's
    /// identity; a directory is removed with its whole subtree first.
    pub fn store_value(&self, path: impl IntoArborPath, value: &Value) -> AdbResult<()> {
        self.store_value_at(&path.into_arbor_path()?, value)
    }

    /// Stores `value` as a file at an already-parsed access path.
    pub(crate) fn store_value_at(&self, path: &APath, value: &Value) -> AdbResult<()> {
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot store a file at the root")));
        }

        self.reindex_around(std::slice::from_ref(path), |table| {
            table::store_value_into(table, path, value)
        })
    }

    /// Sets the scalar at `at` inside the file at `path`. When the new scalar encodes
    /// to the same width as the one already there, the blob is patched in place — no
    /// decode, no re-encode — otherwise the value is decoded, updated, and re-encoded.
    /// Registered indexes are maintained across either path.
    pub(crate) fn put_scalar_at(&self, path: &APath, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        self.reindex_around(std::slice::from_ref(path), |data| {
            table::put_scalar_into(data, path, at, &scalar)
        })
    }

    /// Opens a mutable accessor over the file at `path`, or `None` if absent. A scalar
    /// overwrite through the accessor that keeps the leaf's byte width patches the blob
    /// in place (no decode, no re-encode); a structural change still rewrites it. The
    /// accessor borrows the transaction, so drop it before `commit`.
    pub fn fetch_mut<'t, A: AMut<'t>>(&'t self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        let apath = path.into_arbor_path()?;

        // Presence check without decoding the value — resolve the node and read its
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
        let mut table = self.txn.open_table(data_def(&self.table))?;
        table::ensure_dir(&mut table, &path)?;

        Ok(())
    }

    /// Removes the file or directory at `path` (a directory with its whole
    /// subtree). Returns whether anything was removed.
    pub fn rm(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        let path = path.into_arbor_path()?;
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot remove the root")));
        }

        self.reindex_around(std::slice::from_ref(&path), |dt| table::rm_into(dt, &path))
    }

    /// Moves the node at `src` to `dst`, keeping its identity (a relink, not a
    /// copy — so `mv` is O(1) and any accessor holding the node's `AKey` stays
    /// valid). Creates `dst`'s parent directories and replaces an existing `dst`.
    /// Errors if `dst` is `src` itself or a descendant of it.
    pub fn mv(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        let src = src.into_arbor_path()?;
        let dst = dst.into_arbor_path()?;

        if dst.names().starts_with(src.names()) {
            return Err(AdbError::CannotAccess(String::from(
                "cannot move a node onto itself or into a descendant",
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

        self.reindex_around(&scopes, |dt| table::mv_into(dt, &src, &dst))
    }

    /// Copies the subtree at `src` to `dst` under fresh identities (a deep copy).
    /// Creates `dst`'s parent directories and replaces an existing `dst`. Errors
    /// if `dst` is `src` itself or a descendant of it.
    pub fn cp(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        let src = src.into_arbor_path()?;
        let dst = dst.into_arbor_path()?;

        if dst.names().starts_with(src.names()) {
            return Err(AdbError::CannotAccess(String::from(
                "cannot copy a node onto itself or into a descendant",
            )));
        }

        if dst.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot copy onto the root")));
        }

        // A copy creates fresh entities at `dst`; `src` is unchanged, so only `dst`
        // needs re-indexing.
        self.reindex_around(std::slice::from_ref(&dst), |dt| table::cp_into(dt, &src, &dst))
    }

    /// Loads the table's registered indexes (empty when it has none).
    fn indexes(&self) -> AdbResult<Vec<IndexEntry>> {
        let meta = self.txn.open_table(META_TABLE)?;

        registry::for_table(&meta, &self.table)
    }

    /// Brackets a mutation with index maintenance: remove the affected entities'
    /// current index entries, apply the mutation, then insert the entries the new
    /// state implies. An unindexed table takes the zero-overhead path.
    fn reindex_around<T>(&self, scopes: &[APath], apply: impl FnOnce(&mut DataTable) -> AdbResult<T>) -> AdbResult<T> {
        let indexes = self.indexes()?;
        let mut data = self.txn.open_table(data_def(&self.table))?;

        if indexes.is_empty() {
            return apply(&mut data);
        }

        let mut index = self.txn.open_table(INDEX_TABLE)?;

        for scope in scopes {
            maintenance::delete(&data, &mut index, &indexes, scope)?;
        }

        let result = apply(&mut data)?;

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
    use crate::data::Scalar;
    use crate::index::{registry, IndexColumn, IndexDef};
    use crate::ArborDb;

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
