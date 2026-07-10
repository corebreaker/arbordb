//! The opaque read transaction: a consistent snapshot of one table.

use crate::{
    codec::{decode, ArchivedDir, ArchivedNode, ArchivedValue},
    data::{AValue, Scalar},
    engine::{data_def, entry_kind, read_entry, resolve, split, EntryBytes, EntryKind},
    error::{AdbError, AdbResult},
    node::NodeKind,
    path::{APath, Segment, VPath},
    value::Value,
};

use redb::{ReadOnlyTable, ReadTransaction, TableError};

/// A read transaction over one table — a consistent, concurrent snapshot.
pub struct ReadTxn {
    txn:   ReadTransaction,
    table: String,
}

impl ReadTxn {
    pub(crate) fn new(txn: ReadTransaction, table: String) -> Self {
        Self {
            txn,
            table,
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

    /// Loads the whole [`Value`] stored in the file at `path`, or `None` if there
    /// is nothing there. Errors if `path` names a directory.
    pub fn load(&self, path: impl AsRef<str>) -> AdbResult<Option<Value>> {
        let path = APath::parse(path.as_ref())?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = resolve(&table, &path)? else {
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

    /// Reads the scalar at `at` inside the file at `path`, navigating the value
    /// blob zero-copy. `None` if the file or the inner path is absent, or if the
    /// inner path does not land on a scalar leaf.
    pub fn get(&self, path: impl AsRef<str>, at: impl AsRef<str>) -> AdbResult<Option<Scalar>> {
        let path = APath::parse(path.as_ref())?;
        let at = VPath::parse(at.as_ref())?;

        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = resolve(&table, &path)? else {
            return Ok(None);
        };

        let Some(guard) = table.get(u128::from(akey))? else {
            return Ok(None);
        };

        let (kind, payload) = split(guard.value())?;
        if kind != EntryKind::File {
            return Ok(None);
        }

        let root = ArchivedValue::new(payload)?.root();
        let Some(node) = navigate(root, &at)? else {
            return Ok(None);
        };

        match node.kind()? {
            NodeKind::Leaf => Ok(Some(node.scalar()?)),
            _ => Ok(None),
        }
    }

    /// Reads a typed value at `at` inside the file at `path`.
    pub fn get_as<V: AValue>(&self, path: impl AsRef<str>, at: impl AsRef<str>) -> AdbResult<Option<V>> {
        match self.get(path, at)? {
            Some(scalar) => Ok(Some(V::from_scalar(&scalar)?)),
            None => Ok(None),
        }
    }

    /// The filesystem kind (file or directory) at `path`, or `None` if absent.
    pub fn kind(&self, path: impl AsRef<str>) -> AdbResult<Option<EntryKind>> {
        let path = APath::parse(path.as_ref())?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = resolve(&table, &path)? else {
            return Ok(None);
        };

        entry_kind(&table, akey)
    }

    /// Whether a file or directory exists at `path`.
    pub fn exists(&self, path: impl AsRef<str>) -> AdbResult<bool> {
        Ok(self.kind(path)?.is_some())
    }

    /// Lists the direct children of the directory at `path`, as `(name, kind)`
    /// pairs in name order. Errors if `path` names a file.
    pub fn ls(&self, path: impl AsRef<str>) -> AdbResult<Vec<(String, EntryKind)>> {
        let path = APath::parse(path.as_ref())?;
        let Some(table) = self.open()? else {
            return Ok(Vec::new());
        };

        let Some(akey) = resolve(&table, &path)? else {
            return Err(AdbError::ValueNotFound(path));
        };

        let Some(entry) = read_entry(&table, akey)? else {
            return Ok(Vec::new()); // the root directory, not yet materialized
        };

        let (kind, payload) = split(&entry)?;
        if kind != EntryKind::Dir {
            return Err(AdbError::CannotAccess(format!("'{path}' is a file, not a directory")));
        }

        let dir = ArchivedDir::new(payload)?;
        let mut out = Vec::with_capacity(dir.len()?);
        for (name, child) in dir.entries()? {
            let child_kind = entry_kind(&table, child)?
                .ok_or_else(|| AdbError::Corrupt("a directory entry points at a missing node".into()))?;

            out.push((name.to_string(), child_kind));
        }

        Ok(out)
    }
}

/// Follows a [`VPath`] from `node`, returning the node it lands on, or `None` if a
/// segment leads nowhere.
fn navigate<'a>(mut node: ArchivedNode<'a>, at: &VPath) -> AdbResult<Option<ArchivedNode<'a>>> {
    for segment in at.segments() {
        let next = match segment {
            Segment::Name(name) => node.get(name.as_str())?,
            Segment::Index(index) => node.at(*index as usize)?,
        };

        match next {
            Some(child) => node = child,
            None => return Ok(None),
        }
    }

    Ok(Some(node))
}
