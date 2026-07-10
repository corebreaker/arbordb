use super::{
    super::IndexDef,
    IndexEntry,
    repository::{MetaTable, RegistryRepository},
};

use crate::error::AdbResult;
use redb::ReadableTable;

/// Registers `def` for `table`, returning whether a new index was created.
/// Idempotent (`Ok(false)`) when an identical definition is already registered
/// under the same name; errors with [`AdbError::SchemaMismatch`](crate::AdbError)
/// if the name is taken by a different definition.
pub(crate) fn create(meta: &mut MetaTable<'_>, table: &str, def: &IndexDef) -> AdbResult<bool> {
    RegistryRepository::create(meta, table, def)
}

/// Looks up an index by `table` and `name`.
pub(crate) fn lookup<T: ReadableTable<&'static str, &'static [u8]>>(
    meta: &T,
    table: &str,
    name: &str,
) -> AdbResult<Option<IndexEntry>> {
    RegistryRepository::lookup(meta, table, name)
}

/// Returns every index registered on `table`.
pub(crate) fn for_table<T: ReadableTable<&'static str, &'static [u8]>>(
    meta: &T,
    table: &str,
) -> AdbResult<Vec<IndexEntry>> {
    RegistryRepository::for_table(meta, table)
}

/// Reports whether an index named `name` is registered on `table`, without
/// materializing any [`IndexDef`] (it decodes only each record's table and name).
pub(crate) fn has<T: ReadableTable<&'static str, &'static [u8]>>(meta: &T, table: &str, name: &str) -> AdbResult<bool> {
    RegistryRepository::has(meta, table, name)
}

/// Removes the index named `name` from `table`, returning the removed entry (or
/// `None` if absent) so its physical entries can be purged.
pub(crate) fn delete(meta: &mut MetaTable<'_>, table: &str, name: &str) -> AdbResult<Option<IndexEntry>> {
    RegistryRepository::delete(meta, table, name)
}
