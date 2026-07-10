//! Reserved names and on-disk format constants.

/// The reserved metadata table name (rejected as a user table name).
pub(crate) const METADATA_TABLE_NAME: &str = "$metadata";

/// The metadata key holding the on-disk format version.
pub(crate) const META_FORMAT_VERSION_KEY: &str = "format_version";

/// The metadata key holding the serialized secondary-index registry.
pub(crate) const META_INDEX_REGISTRY_KEY: &str = "index_registry";

/// The on-disk format version this build reads and writes.
pub(crate) const FORMAT_VERSION: u64 = 1;
