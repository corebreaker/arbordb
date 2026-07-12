//! [`RegistryRepository`] — the whole secondary-index registry as one blob under
//! `META_INDEX_REGISTRY_KEY`: a monotonic id allocator plus the table-scoped
//! entries. Every mutation is a read-modify-write of that single value; ids are
//! allocated once and never reused, so a leftover physical entry can never collide
//! with a future index.

use super::{super::IndexId, IndexEntry};
use crate::{
    codec::{self, Reader},
    constants::META_INDEX_REGISTRY_KEY,
    error::{AdbError, AdbResult},
    index::definitions::IndexDef,
};

use redb::{ReadableTable, Table};

/// The write handle to the metadata table.
pub(super) type MetaTable<'txn> = Table<'txn, &'static str, &'static [u8]>;

/// The whole registry: an id allocator plus every registered entry.
#[derive(Default)]
pub(super) struct RegistryRepository {
    /// The next index id to hand out (monotonic; ids are never reused).
    next_id: u32,
    /// Every registered index entry, across all tables.
    entries: Vec<IndexEntry>,
}

impl RegistryRepository {
    /// Decodes the registry blob: the `next_id` allocator, an entry count, then each
    /// `(table, id, def)` record.
    fn decode(data: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(data);
        let next_id = r.u32()?;
        let count = r.u32()?;

        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let table = std::str::from_utf8(r.bytes()?)
                .map(str::to_string)
                .map_err(|_| AdbError::Corrupt("invalid utf-8 in index registry".into()))?;

            let id = IndexId(r.u32()?);
            let def = IndexDef::decode(&mut r)?;
            entries.push(IndexEntry::new(table, id, def));
        }

        Ok(Self {
            next_id,
            entries,
        })
    }

    /// Loads the registry, or an empty one when the metadata key is absent.
    fn load<T: ReadableTable<&'static str, &'static [u8]>>(meta: &T) -> AdbResult<Self> {
        match meta.get(META_INDEX_REGISTRY_KEY)? {
            Some(guard) => Self::decode(guard.value()),
            None => Ok(Self::default()),
        }
    }

    /// Encodes the registry, including its entry count, as a `u32`.
    ///
    /// This caps the registry at `u32::MAX` index entries (across the whole
    /// database, not per table) — far beyond any realistic schema — and is
    /// reported as [`AdbError::Corrupt`] rather than silently truncated.
    fn encode(&self) -> AdbResult<Vec<u8>> {
        let count = u32::try_from(self.entries.len())
            .map_err(|_| AdbError::Corrupt("index registry exceeds u32::MAX entries".into()))?;

        let mut buf = Vec::new();
        codec::put_u32(&mut buf, self.next_id);
        codec::put_u32(&mut buf, count);

        for entry in &self.entries {
            codec::put_bytes(&mut buf, entry.table().as_bytes());
            codec::put_u32(&mut buf, entry.id().0);
            entry.def().encode(&mut buf);
        }

        Ok(buf)
    }

    /// Persists the registry back to its single metadata value.
    fn store(&self, meta: &mut MetaTable<'_>) -> AdbResult<()> {
        meta.insert(META_INDEX_REGISTRY_KEY, self.encode()?.as_slice())?;
        Ok(())
    }

    /// Looks up an index by `table` and `name`.
    pub(crate) fn lookup<T: ReadableTable<&'static str, &'static [u8]>>(
        meta: &T,
        table: &str,
        name: &str,
    ) -> AdbResult<Option<IndexEntry>> {
        let registry = Self::load(meta)?;
        let entry = registry
            .entries
            .into_iter()
            .find(|e| e.table() == table && e.def().name() == name);

        Ok(entry)
    }

    /// Returns every index registered on `table`, in registration order.
    pub(crate) fn for_table<T: ReadableTable<&'static str, &'static [u8]>>(
        meta: &T,
        table: &str,
    ) -> AdbResult<Vec<IndexEntry>> {
        let registry = Self::load(meta)?;
        let entries = registry.entries.into_iter().filter(|e| e.table() == table).collect();

        Ok(entries)
    }

    /// Reports whether an index named `name` is registered on `table`, decoding
    /// only each record's table and index name — never a full [`IndexDef`]. It
    /// walks the registry blob in place (skipping every record's columns and
    /// pattern via [`IndexDef::decode_name`]) and stops at the first match, so it
    /// allocates nothing and parses no column paths.
    pub(crate) fn has<T: ReadableTable<&'static str, &'static [u8]>>(
        meta: &T,
        table: &str,
        name: &str,
    ) -> AdbResult<bool> {
        let Some(guard) = meta.get(META_INDEX_REGISTRY_KEY)? else {
            return Ok(false);
        };

        let mut r = Reader::new(guard.value());
        let _next_id = r.u32()?;
        let count = r.u32()?;

        for _ in 0..count {
            let same_table = std::str::from_utf8(r.bytes()?)
                .map_err(|_| AdbError::Corrupt("invalid utf-8 in index registry".into()))?
                == table;

            let _id = r.u32()?;
            let entry_name = IndexDef::decode_name(&mut r)?;

            if same_table && entry_name == name {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Whether the registry holds any index at all, across every table — reading
    /// only the entry count, never a record. Used at open to prime the write path's
    /// "has any index" fast-path flag, so an index-free database skips the registry
    /// read on every mutation.
    pub(crate) fn any<T: ReadableTable<&'static str, &'static [u8]>>(meta: &T) -> AdbResult<bool> {
        let Some(guard) = meta.get(META_INDEX_REGISTRY_KEY)? else {
            return Ok(false);
        };

        let mut r = Reader::new(guard.value());
        let _next_id = r.u32()?;

        Ok(r.u32()? > 0)
    }

    /// Removes the index named `name` on `table`, returning the entry that was
    /// removed (so its physical entries can be purged) or `None` when no such index
    /// exists. The `next_id` allocator is deliberately left untouched — ids are
    /// never reused, so a stale physical entry can never collide with a future index.
    pub(super) fn delete(meta: &mut MetaTable<'_>, table: &str, name: &str) -> AdbResult<Option<IndexEntry>> {
        let mut registry = Self::load(meta)?;

        let pos = registry
            .entries
            .iter()
            .position(|e| e.table() == table && e.def().name() == name);

        let Some(pos) = pos else {
            return Ok(None);
        };

        let entry = registry.entries.remove(pos);
        registry.store(meta)?;

        Ok(Some(entry))
    }

    /// Registers `def`, returning whether a new index was created (`false` when an
    /// identical one already existed — idempotent).
    pub(super) fn create(meta: &mut MetaTable<'_>, table: &str, def: &IndexDef) -> AdbResult<bool> {
        let mut registry = Self::load(meta)?;
        let entry = registry
            .entries
            .iter()
            .find(|e| e.table() == table && e.def().name() == def.name());

        if let Some(entry) = entry {
            if entry.def() == def {
                return Ok(false);
            }

            return Err(AdbError::SchemaMismatch(format!(
                "index '{name}' on table '{table}' already exists with a different definition",
                name = def.name()
            )));
        }

        let id = IndexId(registry.next_id);
        registry.next_id = registry
            .next_id
            .checked_add(1)
            .ok_or_else(|| AdbError::Corrupt("index registry id counter overflow".into()))?;

        registry
            .entries
            .push(IndexEntry::new(table.to_string(), id, def.clone()));

        registry.store(meta)?;

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::META_TABLE;
    use crate::index::{IndexColumn, IndexDef};
    use crate::path::VPath;

    use redb::{Database, backends::InMemoryBackend};

    fn db() -> Database {
        Database::builder().create_with_backend(InMemoryBackend::new()).unwrap()
    }

    fn def(name: &str, unique: bool) -> IndexDef {
        IndexDef::new(
            String::from(name),
            String::from("users/*"),
            vec![IndexColumn::asc(VPath::root().child_name("age"))],
            unique,
        )
    }

    #[test]
    fn create_is_idempotent_and_allocates_ids() {
        let db = db();
        let txn = db.begin_write().unwrap();

        {
            let mut meta = txn.open_table(META_TABLE).unwrap();

            assert!(RegistryRepository::create(&mut meta, "t", &def("by_age", false)).unwrap());
            // Same definition again: no-op.
            assert!(!RegistryRepository::create(&mut meta, "t", &def("by_age", false)).unwrap());
            // A second index gets the next id.
            assert!(RegistryRepository::create(&mut meta, "t", &def("by_age_u", true)).unwrap());

            assert_eq!(
                RegistryRepository::lookup(&meta, "t", "by_age").unwrap().unwrap().id(),
                IndexId(0)
            );
            assert_eq!(
                RegistryRepository::lookup(&meta, "t", "by_age_u")
                    .unwrap()
                    .unwrap()
                    .id(),
                IndexId(1)
            );
        }

        txn.commit().unwrap();
    }

    #[test]
    fn same_name_different_def_is_a_schema_mismatch() {
        let db = db();
        let txn = db.begin_write().unwrap();

        {
            let mut meta = txn.open_table(META_TABLE).unwrap();

            assert!(RegistryRepository::create(&mut meta, "t", &def("by_age", false)).unwrap());

            let err = RegistryRepository::create(&mut meta, "t", &def("by_age", true)).unwrap_err();
            assert!(matches!(err, AdbError::SchemaMismatch(_)));
        }

        txn.commit().unwrap();
    }

    #[test]
    fn has_for_table_and_delete() {
        let db = db();
        let txn = db.begin_write().unwrap();

        {
            let mut meta = txn.open_table(META_TABLE).unwrap();

            RegistryRepository::create(&mut meta, "t", &def("by_age", false)).unwrap();
            RegistryRepository::create(&mut meta, "t", &def("by_age_u", true)).unwrap();
            // A different table may reuse an index name.
            RegistryRepository::create(&mut meta, "other", &def("by_age", false)).unwrap();

            assert!(RegistryRepository::has(&meta, "t", "by_age").unwrap());
            assert!(!RegistryRepository::has(&meta, "t", "missing").unwrap());
            assert_eq!(RegistryRepository::for_table(&meta, "t").unwrap().len(), 2);

            let removed = RegistryRepository::delete(&mut meta, "t", "by_age").unwrap().unwrap();
            assert_eq!(removed.id(), IndexId(0));
            assert!(!RegistryRepository::has(&meta, "t", "by_age").unwrap());
            assert!(RegistryRepository::delete(&mut meta, "t", "by_age").unwrap().is_none());

            // Ids are never reused: the next index gets id 3, not 0.
            RegistryRepository::create(&mut meta, "t", &def("by_age", false)).unwrap();
            assert_eq!(
                RegistryRepository::lookup(&meta, "t", "by_age").unwrap().unwrap().id(),
                IndexId(3)
            );
        }

        txn.commit().unwrap();
    }
}
