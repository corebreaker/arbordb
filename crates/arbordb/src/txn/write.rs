//! The opaque write transaction: serialized mutation of one table.

use crate::{
    access::MemWriter,
    codec::{encode, encode_dir, ArchivedDir},
    data::AData,
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
    },
    error::{AdbError, AdbResult},
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
        let path = APath::parse(path.as_ref())?;
        let Some((parent_path, name)) = path.split_last() else {
            return Err(AdbError::CannotAccess(String::from("cannot store a file at the root")));
        };

        let mut table = self.txn.open_table(data_def(&self.table))?;
        let parent = ensure_dir(&mut table, &parent_path)?;

        let existing = child_of(&table, parent, name)?;
        let akey = match existing {
            Some(node) => {
                if entry_kind(&table, node)? == Some(EntryKind::File) {
                    node
                } else {
                    cascade_delete(&mut table, node)?;
                    let fresh = AKey::generate();
                    link_child(&mut table, parent, name, fresh)?;
                    fresh
                }
            }
            None => {
                let fresh = AKey::generate();
                link_child(&mut table, parent, name, fresh)?;
                fresh
            }
        };

        let entry = file_entry(&encode(value));
        table.insert(u128::from(akey), entry.as_slice())?;

        Ok(())
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
        let Some((parent_path, name)) = path.split_last() else {
            return Err(AdbError::CannotAccess(String::from("cannot remove the root")));
        };

        let mut table = self.txn.open_table(data_def(&self.table))?;
        let Some(parent) = resolve(&table, &parent_path)? else {
            return Ok(false);
        };

        let Some(akey) = child_of(&table, parent, name)? else {
            return Ok(false);
        };

        cascade_delete(&mut table, akey)?;
        unlink_child(&mut table, parent, name)?;

        Ok(true)
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

        let Some((src_parent_path, src_name)) = src.split_last() else {
            return Err(AdbError::CannotAccess(String::from("cannot move the root")));
        };
        let Some((dst_parent_path, dst_name)) = dst.split_last() else {
            return Err(AdbError::CannotAccess(String::from("cannot move onto the root")));
        };

        let mut table = self.txn.open_table(data_def(&self.table))?;

        let Some(src_parent) = resolve(&table, &src_parent_path)? else {
            return Err(AdbError::ValueNotFound(src));
        };
        let Some(akey) = child_of(&table, src_parent, src_name)? else {
            return Err(AdbError::ValueNotFound(src));
        };

        let dst_parent = ensure_dir(&mut table, &dst_parent_path)?;
        if let Some(existing) = child_of(&table, dst_parent, dst_name)? {
            cascade_delete(&mut table, existing)?;
        }

        link_child(&mut table, dst_parent, dst_name, akey)?;
        unlink_child(&mut table, src_parent, src_name)?;

        Ok(())
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

        let Some((dst_parent_path, dst_name)) = dst.split_last() else {
            return Err(AdbError::CannotAccess(String::from("cannot copy onto the root")));
        };

        let mut table = self.txn.open_table(data_def(&self.table))?;

        let Some(src_akey) = resolve(&table, &src)? else {
            return Err(AdbError::ValueNotFound(src));
        };

        let dst_parent = ensure_dir(&mut table, &dst_parent_path)?;
        if let Some(existing) = child_of(&table, dst_parent, dst_name)? {
            cascade_delete(&mut table, existing)?;
        }

        let copy = deep_copy(&mut table, src_akey)?;
        link_child(&mut table, dst_parent, dst_name, copy)?;

        Ok(())
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
