//! The opaque read transaction: a consistent snapshot of one table.

use crate::{
    cache::PathCache,
    codec::{decode, ArchivedDir, ArchivedNode, ArchivedValue},
    data::{AValue, Scalar},
    engine::{data_def, split, EntryBytes, EntryKind},
    error::{AdbError, AdbResult},
    node::NodeKind,
    path::{APath, Segment, VPath},
    value::Value,
    AKey,
};

use redb::{ReadOnlyTable, ReadTransaction, ReadableTable, TableError};
use std::sync::Arc;

/// A read transaction over one table — a consistent, concurrent snapshot.
///
/// Path resolution and hot-node reads are amortized through the table's shared
/// path cache, tagged with the snapshot's `generation` so a stale entry reads as
/// a miss.
pub struct ReadTxn {
    txn:        ReadTransaction,
    table:      String,
    cache:      Arc<PathCache>,
    generation: u64,
}

impl ReadTxn {
    pub(crate) fn new(txn: ReadTransaction, table: String, cache: Arc<PathCache>, generation: u64) -> Self {
        Self {
            txn,
            table,
            cache,
            generation,
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
    /// engine (and then cached). `None` if the node is absent.
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

    /// Resolves `path` to a node key, consulting the path cache first and walking
    /// directories (through the blob cache) on a miss.
    fn resolve_cached<R>(&self, table: &R, path: &APath) -> AdbResult<Option<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        if let Some(akey) = self.cache.get_path(self.generation, path)? {
            return Ok(Some(akey));
        }

        let mut akey = AKey::ROOT;
        for name in path.names() {
            let Some(blob) = self.entry_blob(table, akey)? else {
                return Ok(None);
            };

            let (kind, payload) = split(&blob)?;
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

    /// Loads the whole [`Value`] stored in the file at `path`, or `None` if there
    /// is nothing there. Errors if `path` names a directory.
    pub fn load(&self, path: impl AsRef<str>) -> AdbResult<Option<Value>> {
        let path = APath::parse(path.as_ref())?;
        let Some(table) = self.open()? else {
            return Ok(None);
        };

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

        let (kind, payload) = split(&blob)?;
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

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

        let (kind, payload) = split(&blob)?;
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

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Ok(None);
        };

        match self.entry_blob(&table, akey)? {
            Some(blob) => Ok(Some(split(&blob)?.0)),
            None => Ok(None),
        }
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

        let Some(akey) = self.resolve_cached(&table, &path)? else {
            return Err(AdbError::ValueNotFound(path));
        };

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(Vec::new()); // the root directory, not yet materialized
        };

        let (kind, payload) = split(&blob)?;
        if kind != EntryKind::Dir {
            return Err(AdbError::CannotAccess(format!("'{path}' is a file, not a directory")));
        }

        let dir = ArchivedDir::new(payload)?;
        let mut out = Vec::with_capacity(dir.len()?);
        for (name, child) in dir.entries()? {
            let child_blob = self
                .entry_blob(&table, child)?
                .ok_or_else(|| AdbError::Corrupt("a directory entry points at a missing node".into()))?;

            out.push((name.to_string(), split(&child_blob)?.0));
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
