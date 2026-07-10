//! The query-side prefix scan over the shared `$index` table.

use super::{IndexId, key};
use crate::{error::AdbResult, AKey};

use redb::ReadableTable;

/// Every entity of index `id` whose encoded columns start with `cols_prefix`, in
/// ascending key order. The matched entity lives in the key tail (non-unique) or
/// in the value (unique).
pub(crate) fn scan_prefix<R>(index: &R, id: IndexId, cols_prefix: &[u8], unique: bool) -> AdbResult<Vec<AKey>>
where
    R: ReadableTable<&'static [u8], &'static [u8]>, {
    let mut lower = key::id_prefix(id).to_vec();
    lower.extend_from_slice(cols_prefix);

    let mut entities = Vec::new();
    for item in index.range(lower.as_slice()..)? {
        let (k, v) = item?;
        let bytes = k.value();

        if !bytes.starts_with(&lower) {
            break;
        }

        let entity = if unique {
            AKey::try_from_bytes(v.value())?
        } else {
            AKey::try_from_bytes(&bytes[bytes.len().saturating_sub(16)..])?
        };

        entities.push(entity);
    }

    Ok(entities)
}
