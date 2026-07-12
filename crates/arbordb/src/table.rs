//! A handle to a single table, from which transactions are started.

use crate::{
    cache::PathCache,
    db::DbInner,
    engine::{self, INDEX_TABLE, META_TABLE},
    error::{AdbError, AdbResult},
    index::{maintenance, registry, AIndexed, IndexDef},
    path::APath,
    txn::{ReadTxn, WriteTxn},
};

use redb::ReadableDatabase;
use std::sync::Arc;

#[cfg(feature = "permissions")]
use crate::perm::Principal;

/// A handle to one named table. Cheap to clone; reads are concurrent, writes are
/// serialized by the engine.
#[derive(Clone)]
pub struct Table {
    /// The database-wide shared state.
    inner: Arc<DbInner>,
    /// This table's name. An `Arc<str>` so each transaction start clones a pointer,
    /// not the string bytes.
    name:  Arc<str>,
    /// This table's shared path/blob cache.
    cache: Arc<PathCache>,

    /// The identity transactions from this handle act as (carried from the
    /// [`ArborDb`](crate::ArborDb) that opened it).
    #[cfg(feature = "permissions")]
    principal: Arc<Principal>,
}

impl Table {
    pub(crate) fn new(
        inner: Arc<DbInner>,
        name: Arc<str>,
        cache: Arc<PathCache>,
        #[cfg(feature = "permissions")] principal: Arc<Principal>,
    ) -> Self {
        Self {
            inner,
            name,
            cache,
            #[cfg(feature = "permissions")]
            principal,
        }
    }

    /// The table's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Begins a read transaction — a consistent snapshot. The snapshot and its
    /// generation are captured atomically against concurrent commits.
    pub fn read(&self) -> AdbResult<ReadTxn> {
        let guard = self
            .inner
            .version_lock()
            .read()
            .map_err(|_| AdbError::CannotAccess(String::from("the version lock was poisoned")))?;

        let txn = self.inner.db().begin_read()?;
        let generation = self.inner.generation();
        drop(guard);

        ReadTxn::new(
            txn,
            self.name.clone(),
            Arc::clone(&self.cache),
            generation,
            #[cfg(feature = "entry-timestamps")]
            Arc::clone(&self.inner),
            #[cfg(feature = "permissions")]
            Arc::clone(&self.principal),
        )
    }

    /// Begins a write transaction (serialized against other writers). The guest
    /// user is read-only and cannot obtain one.
    pub fn write(&self) -> AdbResult<WriteTxn> {
        #[cfg(feature = "permissions")]
        if matches!(self.principal.as_ref(), Principal::Guest { .. }) {
            return Err(AdbError::PermissionDenied(String::from("the guest user is read-only")));
        }

        let txn = self.inner.db().begin_write()?;

        Ok(WriteTxn::new(
            txn,
            self.name.clone(),
            Arc::clone(&self.inner),
            #[cfg(feature = "permissions")]
            Arc::clone(&self.principal),
        ))
    }

    /// Registers a secondary index on this table and back-fills it.
    ///
    /// Idempotent for an identical definition; errors with
    /// [`SchemaMismatch`](AdbError::SchemaMismatch) if `def.name` already names a
    /// different index here. On first creation every pre-existing entity the index
    /// matches is indexed too, so the index is correct whether data was written
    /// before or after — and creating a unique index over duplicate data fails with
    /// [`UniqueViolation`](AdbError::UniqueViolation).
    pub fn create_index(&self, def: &IndexDef) -> AdbResult<()> {
        let txn = self.inner.db().begin_write()?;

        // Register; on a fresh creation recover the entry (it carries the new id)
        // so the back-fill below can build its keys.
        let new_entry = {
            let mut meta = txn.open_table(META_TABLE)?;
            if registry::create(&mut meta, &self.name, def)? {
                registry::lookup(&meta, &self.name, def.name())?
            } else {
                None
            }
        };

        self.backfill(&txn, new_entry)?;
        txn.commit()?;
        self.inner.mark_has_index();

        Ok(())
    }

    /// Registers every index that `T` declares (via `#[arbor(index(...))]`) on this
    /// table, scoping each to `pattern`, and back-fills them. A shorthand for
    /// [`create_index`](Self::create_index) on each of
    /// [`T::index_defs`](AIndexed::index_defs).
    ///
    /// `pattern` selects which nodes are the indexed *entities*: a slash-separated
    /// access path where `*` matches any single child and every other segment
    /// matches literally (`users/*` indexes every direct child of `users`). It is
    /// not recursive, and the empty string `""` scopes the index to the table root.
    pub fn create_indexes<T: AIndexed>(&self, pattern: &str) -> AdbResult<()> {
        for def in T::index_defs(pattern) {
            self.create_index(&def)?;
        }

        Ok(())
    }

    /// Registers `def` and back-fills it **only if no index of that name already
    /// exists** here; otherwise a no-op, leaving the present index untouched (its
    /// definition is *not* reconciled with `def`). The idempotent-by-name
    /// counterpart of [`create_index`](Self::create_index).
    pub fn ensure_index(&self, def: &IndexDef) -> AdbResult<()> {
        let txn = self.inner.db().begin_write()?;

        let new_entry = {
            let mut meta = txn.open_table(META_TABLE)?;
            if registry::has(&meta, &self.name, def.name())? {
                None
            } else {
                registry::create(&mut meta, &self.name, def)?;
                registry::lookup(&meta, &self.name, def.name())?
            }
        };

        self.backfill(&txn, new_entry)?;
        txn.commit()?;
        self.inner.mark_has_index();

        Ok(())
    }

    /// Ensures every index that `T` declares exists here, scoping each to `pattern`.
    /// The idempotent-by-name counterpart of
    /// [`create_indexes`](Self::create_indexes).
    pub fn ensure_indexes<T: AIndexed>(&self, pattern: &str) -> AdbResult<()> {
        for def in T::index_defs(pattern) {
            self.ensure_index(&def)?;
        }

        Ok(())
    }

    /// The definition of the named index on this table, if it exists.
    pub fn index_def(&self, name: &str) -> AdbResult<Option<IndexDef>> {
        let txn = self.inner.db().begin_read()?;
        let meta = txn.open_table(META_TABLE)?;

        Ok(registry::lookup(&meta, &self.name, name)?.map(|entry| entry.into_def()))
    }

    /// Whether an index named `name` is registered on this table (decodes only each
    /// record's table and name, never a full definition).
    pub fn has_index(&self, name: &str) -> AdbResult<bool> {
        let txn = self.inner.db().begin_read()?;
        let meta = txn.open_table(META_TABLE)?;

        registry::has(&meta, &self.name, name)
    }

    /// Drops the index named `name`: in one transaction it removes the registry
    /// record and purges every entry the index holds. Returns whether an index was
    /// actually removed (`false`, idempotent, when none of that name exists).
    pub fn delete_index(&self, name: &str) -> AdbResult<bool> {
        let txn = self.inner.db().begin_write()?;

        // Drop the registry record first; the recovered entry carries the id the
        // physical purge below needs.
        let removed = {
            let mut meta = txn.open_table(META_TABLE)?;
            registry::delete(&mut meta, &self.name, name)?
        };

        let found = removed.is_some();
        if let Some(entry) = removed {
            let mut index = txn.open_table(INDEX_TABLE)?;
            maintenance::delete_all(&mut index, &entry)?;
        }

        txn.commit()?;

        Ok(found)
    }

    /// Drops every index that `T` declares from this table, returning how many were
    /// actually removed. An index name is fixed by the derive independent of scope,
    /// so no pattern is needed; each drop is idempotent.
    pub fn delete_indexes<T: AIndexed>(&self) -> AdbResult<usize> {
        let mut removed = 0;
        for def in T::index_defs("") {
            if self.delete_index(def.name())? {
                removed += 1;
            }
        }

        Ok(removed)
    }

    /// Back-fills a freshly-registered index: indexes every pre-existing entity it
    /// matches, reading from the data table into the shared index table.
    fn backfill(&self, txn: &redb::WriteTransaction, entry: Option<registry::IndexEntry>) -> AdbResult<()> {
        let Some(entry) = entry else {
            return Ok(());
        };

        let data = txn.open_table(engine::data_def(&self.name))?;
        let mut index = txn.open_table(INDEX_TABLE)?;

        maintenance::insert(&data, &mut index, std::slice::from_ref(&entry), &APath::root())
    }
}
