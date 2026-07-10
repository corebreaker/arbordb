//! The physical key layout of a secondary-index entry in the shared `$index` table.
//!
//! Every entry begins with the index's globally-unique 4-byte id, so one table
//! holds every index's entries and each index occupies a contiguous key block.
//! A non-unique index appends the entity key as a tie-breaker after the columns;
//! a unique index omits it (the columns alone form the key) and carries the
//! entity in the value instead, so a duplicate column tuple is a key collision.

use super::IndexId;
use crate::AKey;

/// The 4-byte big-endian prefix shared by every entry of index `id`.
pub(crate) fn id_prefix(id: IndexId) -> [u8; 4] {
    id.0.to_be_bytes()
}

/// The physical key of an index entry: `id · cols`, plus the entity key for a
/// non-unique index.
pub(crate) fn entry_key(id: IndexId, cols: &[u8], entity: Option<AKey>) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + cols.len() + if entity.is_some() { 16 } else { 0 });
    key.extend_from_slice(&id_prefix(id));
    key.extend_from_slice(cols);

    if let Some(akey) = entity {
        key.extend_from_slice(&akey.into_bytes());
    }

    key
}
