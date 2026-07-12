//! The set of optional on-disk features a database file requires a reader to
//! understand.
//!
//! A file records a feature name here only when opening it *without* that feature
//! would be unsafe — for example `permissions`, which access-controls the file.
//! Features that degrade gracefully (a reader can simply ignore their data, as
//! with `entry-timestamps`) are never recorded, so an unaware binary opens the
//! file and quietly ignores them.
//!
//! The record is one metadata blob under [`META_REQUIRED_FEATURES_KEY`]: a `u32`
//! count followed by that many length-prefixed UTF-8 feature names.

use crate::{
    codec::Reader,
    constants::META_REQUIRED_FEATURES_KEY,
    error::{AdbError, AdbResult},
};

use redb::ReadableTable;

/// Whether this build was compiled with the named optional feature.
fn is_compiled(name: &str) -> bool {
    let permissions = cfg!(feature = "permissions");
    let timestamps = cfg!(feature = "entry-timestamps");

    match name {
        "permissions" => permissions,
        "entry-timestamps" => timestamps,
        _ => false,
    }
}

/// Decodes the recorded feature names (a `u32` count then that many
/// length-prefixed UTF-8 strings).
fn decode(bytes: &[u8]) -> AdbResult<Vec<String>> {
    let mut r = Reader::new(bytes);
    let count = r.u32()?;

    let mut names = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = std::str::from_utf8(r.bytes()?)
            .map(str::to_string)
            .map_err(|_| AdbError::Corrupt("invalid utf-8 in the required-features record".into()))?;

        names.push(name);
    }

    Ok(names)
}

/// The feature names `meta` records as required (empty when the key is absent).
pub(crate) fn list<T>(meta: &T) -> AdbResult<Vec<String>>
where
    T: ReadableTable<&'static str, &'static [u8]>, {
    match meta.get(META_REQUIRED_FEATURES_KEY)? {
        Some(guard) => decode(guard.value()),
        None => Ok(Vec::new()),
    }
}

/// Rejects the database when it requires an optional feature this build lacks.
///
/// A missing `permissions` feature surfaces as [`AdbError::DatabaseProtected`]
/// (the file is protected and cannot be opened); any other missing feature as
/// [`AdbError::FeatureRequired`].
pub(crate) fn check<T>(meta: &T) -> AdbResult<()>
where
    T: ReadableTable<&'static str, &'static [u8]>, {
    for name in list(meta)? {
        if !is_compiled(&name) {
            if name == "permissions" {
                return Err(AdbError::DatabaseProtected);
            }

            return Err(AdbError::FeatureRequired {
                feature: name
            });
        }
    }

    Ok(())
}

/// Records `name` as a required feature (idempotent) — used when a feature first
/// makes a database unreadable by binaries built without it.
#[cfg(feature = "permissions")]
pub(crate) fn require(meta: &mut redb::Table<'_, &'static str, &'static [u8]>, name: &str) -> AdbResult<()> {
    let mut names = list(&*meta)?;
    if names.iter().any(|existing| existing == name) {
        return Ok(());
    }

    names.push(name.to_string());

    let mut buf = Vec::new();
    crate::codec::put_u32(&mut buf, names.len() as u32);
    for feature in &names {
        crate::codec::put_bytes(&mut buf, feature.as_bytes());
    }

    meta.insert(META_REQUIRED_FEATURES_KEY, buf.as_slice())?;

    Ok(())
}
