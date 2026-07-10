//! Write-time index maintenance.
//!
//! A mutation keeps every index in sync by re-indexing the entities it could have
//! affected — those the mutation's access path passes through (see
//! [`Pattern::affected_entities`]). The caller brackets the mutation: [`delete`]
//! the affected entities' current entries *before* the change, apply the change,
//! then [`insert`] the entries the new state implies. Because an entry's key is a
//! pure function of the entity's column values and key, this delete-then-insert
//! cleanly covers creation, replacement, column edits, and removal without diffing.
//!
//! A missing or non-leaf column yields `Null`, so an entity always produces a
//! well-formed key.

use super::{IndexId, definitions::IndexDef, key, pattern::Pattern, registry::IndexEntry};
use crate::{
    codec::ArchivedValue,
    data::Scalar,
    engine::{entry_split, EntryBytes, EntryKind, read_entry},
    error::{AdbError, AdbResult},
    node::NodeKind,
    path::{APath, VPath},
    AKey,
};

use redb::{ReadableTable, Table};

/// The writable `$index` table.
pub(crate) type IndexTable<'txn> = Table<'txn, &'static [u8], &'static [u8]>;

/// Removes the index entries the affected entities currently have. Call before
/// applying a mutation at `scope`, reading entity columns from `data`.
pub(crate) fn delete<R>(data: &R, index: &mut IndexTable<'_>, indexes: &[IndexEntry], scope: &APath) -> AdbResult<()>
where
    R: ReadableTable<u128, EntryBytes>, {
    for entry in indexes {
        for (k, _) in entries_for(data, entry, scope)? {
            index.remove(k.as_slice())?;
        }
    }

    Ok(())
}

/// Inserts the index entries the affected entities now imply. Call after applying
/// a mutation at `scope`. A unique index rejects an entry whose columns already map
/// to a different entity with [`AdbError::UniqueViolation`].
pub(crate) fn insert<R>(data: &R, index: &mut IndexTable<'_>, indexes: &[IndexEntry], scope: &APath) -> AdbResult<()>
where
    R: ReadableTable<u128, EntryBytes>, {
    for entry in indexes {
        let unique = entry.def().unique();

        for (k, v) in entries_for(data, entry, scope)? {
            if unique {
                guard_unique(index, &k, &v, entry.def().name())?;
            }

            index.insert(k.as_slice(), v.as_slice())?;
        }
    }

    Ok(())
}

/// Removes every physical entry belonging to `entry`, across all indexed entities.
/// Call when an index is dropped. Its entries occupy one contiguous key block (the
/// same leading id), so a single forward range scan from that id's lower bound,
/// stopping at the first differing id, covers exactly this index (ids are never
/// reused, so no other index shares one).
pub(crate) fn delete_all(index: &mut IndexTable<'_>, entry: &IndexEntry) -> AdbResult<()> {
    let prefix = key::id_prefix(entry.id());

    let mut keys = Vec::new();
    for item in index.range(prefix.as_slice()..)? {
        let (k, _) = item?;
        let bytes = k.value();

        if !bytes.starts_with(&prefix) {
            break;
        }

        keys.push(bytes.to_vec());
    }

    for k in keys {
        index.remove(k.as_slice())?;
    }

    Ok(())
}

/// Fails with [`AdbError::UniqueViolation`] if `key` already maps to an entity
/// other than the one in `value`. Re-inserting an entity's own entry is fine —
/// `delete` removes it first — so this fires only on a genuine collision.
fn guard_unique(index: &IndexTable<'_>, key: &[u8], value: &[u8], name: &str) -> AdbResult<()> {
    if let Some(existing) = index.get(key)?
        && existing.value() != value
    {
        return Err(AdbError::UniqueViolation {
            index: name.to_string(),
        });
    }

    Ok(())
}

/// The (key, value) index entries for every entity `entry` matches on `scope`'s line.
fn entries_for<R>(data: &R, entry: &IndexEntry, scope: &APath) -> AdbResult<Vec<(Vec<u8>, Vec<u8>)>>
where
    R: ReadableTable<u128, EntryBytes>, {
    let pattern = Pattern::parse(entry.def().pattern())?;

    pattern
        .affected_entities(data, scope)?
        .into_iter()
        .map(|entity| index_key(data, entry.id(), entry.def(), entity))
        .collect()
}

/// Builds the (key, value) an index entry for `entity` takes: a non-unique index
/// puts the entity key in the table key (a tie-breaker after the columns) with an
/// empty value; a unique index puts it in the value, so the columns alone key it.
fn index_key<R>(data: &R, id: IndexId, def: &IndexDef, entity: AKey) -> AdbResult<(Vec<u8>, Vec<u8>)>
where
    R: ReadableTable<u128, EntryBytes>, {
    let values = def
        .columns()
        .iter()
        .map(|column| column_scalar(data, entity, column.path()))
        .collect::<AdbResult<Vec<_>>>()?;
    let cols = def.encode_columns(&values);

    if def.unique() {
        Ok((key::entry_key(id, &cols, None), entity.into_bytes().to_vec()))
    } else {
        Ok((key::entry_key(id, &cols, Some(entity)), Vec::new()))
    }
}

/// The scalar at `column` relative to `entity`, or `Null` when the entity is
/// absent / not a file, or the path is absent / not a leaf.
fn column_scalar<R>(data: &R, entity: AKey, column: &VPath) -> AdbResult<Scalar>
where
    R: ReadableTable<u128, EntryBytes>, {
    let Some(entry) = read_entry(data, entity)? else {
        return Ok(Scalar::Null);
    };

    let (kind, payload) = entry_split(&entry)?;
    if kind != EntryKind::File {
        return Ok(Scalar::Null);
    }

    let Some(node) = ArchivedValue::new(payload)?.root().navigate(column)? else {
        return Ok(Scalar::Null);
    };

    match node.kind()? {
        NodeKind::Leaf => node.scalar(),
        _ => Ok(Scalar::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{encode, encode_dir};
    use crate::engine::{data_def, dir_entry, file_entry, INDEX_TABLE, META_TABLE};
    use crate::index::{registry, IndexColumn, IndexDef};
    use crate::value::Value;

    use redb::{Database, backends::InMemoryBackend};
    use std::collections::BTreeMap;

    fn user(age: i64) -> Value {
        Value::Node(BTreeMap::from([(String::from("age"), Value::Leaf(Scalar::I64(age)))]))
    }

    fn by_age(unique: bool) -> IndexDef {
        IndexDef::new(
            String::from("by_age"),
            String::from("*"),
            vec![IndexColumn::asc(VPath::root().child_name("age"))],
            unique,
        )
    }

    /// Counts the physical entries of index `id`.
    fn count(index: &IndexTable<'_>, id: IndexId) -> usize {
        let prefix = key::id_prefix(id);
        index
            .range(prefix.as_slice()..)
            .unwrap()
            .map(|item| item.unwrap().0.value().to_vec())
            .take_while(|k| k.starts_with(&prefix))
            .count()
    }

    #[test]
    fn maintenance_tracks_store_edit_and_remove() {
        let db = Database::builder().create_with_backend(InMemoryBackend::new()).unwrap();
        let txn = db.begin_write().unwrap();

        {
            let mut meta = txn.open_table(META_TABLE).unwrap();
            registry::create(&mut meta, "t", &by_age(false)).unwrap();
            let indexes = registry::for_table(&meta, "t").unwrap();
            let id = indexes[0].id();

            let mut data = txn.open_table(data_def("t")).unwrap();
            let (alice, bob) = (AKey::generate(), AKey::generate());
            data.insert(u128::from(alice), file_entry(&encode(&user(30))).as_slice())
                .unwrap();
            data.insert(u128::from(bob), file_entry(&encode(&user(40))).as_slice())
                .unwrap();

            let root = BTreeMap::from([(String::from("alice"), alice), (String::from("bob"), bob)]);
            data.insert(u128::from(AKey::ROOT), dir_entry(&encode_dir(&root)).as_slice())
                .unwrap();

            let mut index = txn.open_table(INDEX_TABLE).unwrap();

            // Back-fill from the root: both entities get an entry.
            insert(&data, &mut index, &indexes, &APath::root()).unwrap();
            assert_eq!(count(&index, id), 2);

            // Edit alice's age: delete the old entry, rewrite the blob, insert the new.
            delete(&data, &mut index, &indexes, &APath::parse("alice").unwrap()).unwrap();
            data.insert(u128::from(alice), file_entry(&encode(&user(31))).as_slice())
                .unwrap();
            insert(&data, &mut index, &indexes, &APath::parse("alice").unwrap()).unwrap();
            assert_eq!(count(&index, id), 2);

            // Remove bob: delete its entry before unlinking.
            delete(&data, &mut index, &indexes, &APath::parse("bob").unwrap()).unwrap();
            let mut root = root.clone();
            root.remove("bob");
            data.insert(u128::from(AKey::ROOT), dir_entry(&encode_dir(&root)).as_slice())
                .unwrap();
            assert_eq!(count(&index, id), 1);
        }

        txn.commit().unwrap();
    }

    #[test]
    fn a_unique_index_rejects_a_duplicate_column_value() {
        let db = Database::builder().create_with_backend(InMemoryBackend::new()).unwrap();
        let txn = db.begin_write().unwrap();

        {
            let mut meta = txn.open_table(META_TABLE).unwrap();
            registry::create(&mut meta, "t", &by_age(true)).unwrap();
            let indexes = registry::for_table(&meta, "t").unwrap();

            let mut data = txn.open_table(data_def("t")).unwrap();
            let (alice, bob) = (AKey::generate(), AKey::generate());
            // Both users share age 30.
            data.insert(u128::from(alice), file_entry(&encode(&user(30))).as_slice())
                .unwrap();
            data.insert(u128::from(bob), file_entry(&encode(&user(30))).as_slice())
                .unwrap();

            let root = BTreeMap::from([(String::from("alice"), alice), (String::from("bob"), bob)]);
            data.insert(u128::from(AKey::ROOT), dir_entry(&encode_dir(&root)).as_slice())
                .unwrap();

            let mut index = txn.open_table(INDEX_TABLE).unwrap();

            let err = insert(&data, &mut index, &indexes, &APath::root()).unwrap_err();
            assert!(matches!(err, AdbError::UniqueViolation { .. }));
        }
    }
}
