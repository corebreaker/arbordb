//! The opaque write transaction: serialized mutation of one table.

use crate::{
    codec::{encode, encode_dir, ArchivedDir},
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
    path::APath,
    value::Value,
    AKey,
};

use redb::{ReadableTable, WriteTransaction};
use std::collections::BTreeMap;

/// The per-transaction data table (borrows the write transaction).
type DataTable<'txn> = redb::Table<'txn, u128, EntryBytes>;

/// A write transaction over one table. Changes become durable on [`commit`](WriteTxn::commit).
pub struct WriteTxn {
    txn:   WriteTransaction,
    table: String,
}

impl WriteTxn {
    pub(crate) fn new(txn: WriteTransaction, table: String) -> Self {
        Self {
            txn,
            table,
        }
    }

    /// Stores `value` as a file at `path`, creating parent directories as needed.
    /// Replaces whatever was there: a file overwrite keeps the node's identity; a
    /// directory is removed with its whole subtree first.
    pub fn store(&self, path: impl AsRef<str>, value: &Value) -> AdbResult<()> {
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

    /// Commits the transaction, making its changes durable.
    pub fn commit(self) -> AdbResult<()> {
        self.txn.commit()?;

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
