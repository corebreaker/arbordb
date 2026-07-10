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
    node::NodeKind,
    path::{APath, IntoArborPath, IntoValuePath, VPath},
    value::Value,
    AKey,
};

use redb::{ReadOnlyTable, ReadTransaction, ReadableTable, TableError};
use std::{collections::HashSet, sync::Arc};

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

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

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

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

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

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

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

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(None);
        };

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
        let Some(blob) = self.entry_blob(table, entity)? else {
            return Ok(None);
        };

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

        let Some(blob) = self.entry_blob(&table, akey)? else {
            return Ok(Vec::new()); // the root directory, not yet materialized
        };

        let (kind, payload) = entry_split(&blob)?;
        if kind != EntryKind::Dir {
            return Err(AdbError::CannotAccess(format!("'{path}' is a file, not a directory")));
        }

        let dir = ArchivedDir::new(payload)?;
        let mut out = Vec::with_capacity(dir.len()?);
        for (name, child) in dir.entries()? {
            let child_blob = self
                .entry_blob(&table, child)?
                .ok_or_else(|| AdbError::Corrupt("a directory entry points at a missing node".into()))?;

            out.push(Entry::new(name.to_string(), get_entry_kind(&child_blob)?));
        }

        Ok(out)
    }
}
