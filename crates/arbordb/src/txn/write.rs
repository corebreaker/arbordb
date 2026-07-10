//! The opaque write transaction: serialized mutation of one table.

use crate::{
    access::{MemWriter, MutCursor, Writer},
    codec::{decode, encode, encode_dir, ArchivedDir},
    data::{AData, AMut},
    db::DbInner,
    engine::{
        child_of,
        data_def,
        dir_entry,
        entry_kind,
        file_entry,
        read_entry,
        resolve,
        split,
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
    path::{APath, VPath},
    value::Value,
    AKey,
};

use redb::{ReadableTable, WriteTransaction};
use std::{collections::BTreeMap, sync::Arc};

/// The per-transaction data table (borrows the write transaction).
type DataTable<'txn> = redb::Table<'txn, u128, EntryBytes>;

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
    pub fn store<T: AData>(&self, path: impl AsRef<str>, value: &T) -> AdbResult<()> {
        let writer = MemWriter::new();
        value.store(&writer, &VPath::root())?;

        self.store_value(path, &writer.into_value())
    }

    /// Stores a dynamic [`Value`] as a file at `path`, creating parent directories
    /// as needed. Replaces whatever was there: a file overwrite keeps the node's
    /// identity; a directory is removed with its whole subtree first.
    pub fn store_value(&self, path: impl AsRef<str>, value: &Value) -> AdbResult<()> {
        self.store_value_at(&APath::parse(path.as_ref())?, value)
    }

    /// Stores `value` as a file at an already-parsed access path.
    pub(crate) fn store_value_at(&self, path: &APath, value: &Value) -> AdbResult<()> {
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot store a file at the root")));
        }

        self.reindex_around(std::slice::from_ref(path), |table| store_value_into(table, path, value))
    }

    /// Opens a mutable accessor over the file at `path`, or `None` if absent. Each
    /// mutation through the accessor re-encodes and rewrites the file's blob (the
    /// accepted O(blob) cost of a partial write). The accessor borrows the
    /// transaction, so drop it before `commit`.
    pub fn fetch_mut<'t, A: AMut<'t>>(&'t self, path: impl AsRef<str>) -> AdbResult<Option<A>> {
        let apath = APath::parse(path.as_ref())?;
        if self.load_value_at(&apath)?.is_none() {
            return Ok(None);
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

        let (kind, payload) = split(&entry)?;
        match kind {
            EntryKind::File => Ok(Some(decode(payload)?)),
            EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
        }
    }

    /// Creates the directory at `path` (and any missing ancestors). Idempotent;
    /// errors if a path component is an existing file.
    pub fn mkdir(&self, path: impl AsRef<str>) -> AdbResult<()> {
        let path = APath::parse(path.as_ref())?;
        let mut table = self.txn.open_table(data_def(&self.table))?;
        ensure_dir(&mut table, &path)?;

        Ok(())
    }

    /// Removes the file or directory at `path` (a directory with its whole
    /// subtree). Returns whether anything was removed.
    pub fn rm(&self, path: impl AsRef<str>) -> AdbResult<bool> {
        let path = APath::parse(path.as_ref())?;
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot remove the root")));
        }

        self.reindex_around(std::slice::from_ref(&path), |table| rm_into(table, &path))
    }

    /// Moves the node at `src` to `dst`, keeping its identity (a relink, not a
    /// copy — so `mv` is O(1) and any accessor holding the node's `AKey` stays
    /// valid). Creates `dst`'s parent directories and replaces an existing `dst`.
    /// Errors if `dst` is `src` itself or a descendant of it.
    pub fn mv(&self, src: impl AsRef<str>, dst: impl AsRef<str>) -> AdbResult<()> {
        let src = APath::parse(src.as_ref())?;
        let dst = APath::parse(dst.as_ref())?;

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

        self.reindex_around(&scopes, |table| mv_into(table, &src, &dst))
    }

    /// Copies the subtree at `src` to `dst` under fresh identities (a deep copy).
    /// Creates `dst`'s parent directories and replaces an existing `dst`. Errors
    /// if `dst` is `src` itself or a descendant of it.
    pub fn cp(&self, src: impl AsRef<str>, dst: impl AsRef<str>) -> AdbResult<()> {
        let src = APath::parse(src.as_ref())?;
        let dst = APath::parse(dst.as_ref())?;

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
        self.reindex_around(std::slice::from_ref(&dst), |table| cp_into(table, &src, &dst))
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

/// Stores `value` as a file at `path` (a non-root path), creating parents and
/// replacing whatever was there.
fn store_value_into(table: &mut DataTable, path: &APath, value: &Value) -> AdbResult<()> {
    let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");
    let parent = ensure_dir(table, &parent_path)?;

    let akey = match child_of(&*table, parent, name)? {
        Some(node) => {
            if entry_kind(&*table, node)? == Some(EntryKind::File) {
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

/// Removes the node at `path` (a non-root path) and its subtree. Returns whether
/// anything was removed.
fn rm_into(table: &mut DataTable, path: &APath) -> AdbResult<bool> {
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
fn mv_into(table: &mut DataTable, src: &APath, dst: &APath) -> AdbResult<()> {
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
fn cp_into(table: &mut DataTable, src: &APath, dst: &APath) -> AdbResult<()> {
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
fn read_dir_map<R>(table: &R, akey: AKey) -> AdbResult<BTreeMap<String, AKey>>
where
    R: ReadableTable<u128, EntryBytes>, {
    let Some(entry) = read_entry(table, akey)? else {
        return Ok(BTreeMap::new());
    };

    let (kind, payload) = split(&entry)?;
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
fn put_dir(table: &mut DataTable, akey: AKey, map: &BTreeMap<String, AKey>) -> AdbResult<()> {
    let entry = dir_entry(&encode_dir(map));
    table.insert(u128::from(akey), entry.as_slice())?;

    Ok(())
}

/// Adds (or replaces) a `name → child` link in directory `parent`.
fn link_child(table: &mut DataTable, parent: AKey, name: &str, child: AKey) -> AdbResult<()> {
    let mut map = read_dir_map(&*table, parent)?;
    map.insert(name.to_string(), child);

    put_dir(table, parent, &map)
}

/// Removes the `name` link from directory `parent`.
fn unlink_child(table: &mut DataTable, parent: AKey, name: &str) -> AdbResult<()> {
    let mut map = read_dir_map(&*table, parent)?;
    map.remove(name);

    put_dir(table, parent, &map)
}

/// Ensures the root directory exists.
fn ensure_root(table: &mut DataTable) -> AdbResult<()> {
    if read_entry(&*table, AKey::ROOT)?.is_none() {
        put_dir(table, AKey::ROOT, &BTreeMap::new())?;
    }

    Ok(())
}

/// Ensures every directory along `path` exists, returning the deepest one's key.
fn ensure_dir(table: &mut DataTable, path: &APath) -> AdbResult<AKey> {
    ensure_root(table)?;

    let mut akey = AKey::ROOT;
    for name in path.names() {
        match child_of(&*table, akey, name.as_str())? {
            Some(child) => {
                if entry_kind(&*table, child)? != Some(EntryKind::Dir) {
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
fn cascade_delete(table: &mut DataTable, akey: AKey) -> AdbResult<()> {
    let children = match read_entry(&*table, akey)? {
        Some(entry) => {
            let (kind, payload) = split(&entry)?;
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
fn deep_copy(table: &mut DataTable, akey: AKey) -> AdbResult<AKey> {
    let entry = read_entry(&*table, akey)?.ok_or_else(|| AdbError::Corrupt("copying a missing node".into()))?;
    let (kind, payload) = split(&entry)?;
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
}
