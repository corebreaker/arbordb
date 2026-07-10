//! The storage-engine boundary: redb table definitions, metadata bootstrap, and
//! the read-side directory-tree walk. The concrete engine (redb) is confined to
//! this layer.

use super::{
    entry::{get_entry_kind, entry_split, EntryKind},
    features,
};

use crate::{
    codec::ArchivedDir,
    constants::{FORMAT_VERSION, INDEX_TABLE_NAME, META_FORMAT_VERSION_KEY, METADATA_TABLE_NAME},
    error::{AdbError, AdbResult},
    path::APath,
    AKey,
};

use redb::{Database, ReadableTable, TableDefinition};

/// The value type of a data table: a vnode's entry blob. redb hands back a slice
/// borrowing the page, which is exactly the zero-copy read path.
pub(crate) type EntryBytes = &'static [u8];

/// The reserved metadata table: small keyed blobs — the format version and the
/// secondary-index registry. Byte-valued so it can hold either.
pub(crate) const META_TABLE: TableDefinition<&str, EntryBytes> = TableDefinition::new(METADATA_TABLE_NAME);

/// The per-table data table: `AKey` (as `u128`) → entry blob (a directory or a file).
pub(crate) fn data_def(name: &str) -> TableDefinition<'_, u128, EntryBytes> {
    TableDefinition::new(name)
}

/// The shared secondary-index table: an order-preserving byte key
/// (`index-id · encoded-columns · entity`) → an entity key (unique) or nothing.
pub(crate) const INDEX_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new(INDEX_TABLE_NAME);

/// Opens (creating if needed) the metadata table and checks the format version,
/// writing it on a fresh database.
pub(crate) fn bootstrap_metadata(db: &Database) -> AdbResult<()> {
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;

        let current = match meta.get(META_FORMAT_VERSION_KEY)? {
            Some(guard) => {
                let bytes: [u8; 8] = guard
                    .value()
                    .try_into()
                    .map_err(|_| AdbError::Corrupt("invalid on-disk format version".into()))?;

                Some(u64::from_be_bytes(bytes))
            }
            None => None,
        };

        match current {
            Some(version) if version > FORMAT_VERSION => {
                return Err(AdbError::SchemaMismatch(format!(
                    "on-disk format version {version} is newer than this build's {FORMAT_VERSION}"
                )));
            }
            Some(_) => {}
            None => {
                meta.insert(META_FORMAT_VERSION_KEY, FORMAT_VERSION.to_be_bytes().as_slice())?;
            }
        }

        // Reject a file that requires an optional feature this build lacks — e.g. a
        // protected (`permissions`) file opened by a binary compiled without it.
        // Gracefully-degrading features are never recorded, so this never rejects them.
        features::check(&meta)?;
    }
    txn.commit()?;

    Ok(())
}

/// Reads a vnode's raw entry blob (an owned copy), or `None` if absent.
pub(crate) fn read_entry<R>(table: &R, akey: AKey) -> AdbResult<Option<Vec<u8>>>
where
    R: ReadableTable<u128, EntryBytes>, {
    Ok(table.get(u128::from(akey))?.map(|guard| guard.value().to_vec()))
}

/// The filesystem kind of vnode `akey`, or `None` if absent.
pub(crate) fn fetch_entry_kind<R>(table: &R, akey: AKey) -> AdbResult<Option<EntryKind>>
where
    R: ReadableTable<u128, EntryBytes>, {
    match read_entry(table, akey)? {
        Some(entry) => Ok(Some(get_entry_kind(&entry)?)),
        None => Ok(None),
    }
}

/// The child key under directory `parent` named `name`, or `None` if `parent` is
/// absent, is not a directory, or has no such child.
pub(crate) fn child_of<R>(table: &R, parent: AKey, name: &str) -> AdbResult<Option<AKey>>
where
    R: ReadableTable<u128, EntryBytes>, {
    let Some(entry) = read_entry(table, parent)? else {
        return Ok(None);
    };

    let (kind, payload) = entry_split(&entry)?;
    if kind != EntryKind::Dir {
        return Ok(None);
    }

    ArchivedDir::new(payload)?.get(name)
}

/// Resolves `path` to its vnode key by walking directories from [`AKey::ROOT`], or
/// `None` if any segment along the way is missing.
pub(crate) fn resolve<R>(table: &R, path: &APath) -> AdbResult<Option<AKey>>
where
    R: ReadableTable<u128, EntryBytes>, {
    let mut akey = AKey::ROOT;
    for name in path.names() {
        match child_of(table, akey, name.as_str())? {
            Some(child) => akey = child,
            None => return Ok(None),
        }
    }

    Ok(Some(akey))
}
