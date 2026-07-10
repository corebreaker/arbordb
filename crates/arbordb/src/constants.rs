//! Reserved names and on-disk format constants.

/// The reserved metadata table name (rejected as a user table name).
pub(crate) const METADATA_TABLE_NAME: &str = "$metadata";

/// The reserved secondary-index table name (rejected as a user table name). One
/// table holds every index's entries, keyed by the globally-unique index id.
pub(crate) const INDEX_TABLE_NAME: &str = "$index";

/// The reserved per-vnode metadata table name (created/modified/accessed times and,
/// with `permissions`, ACLs). Keyed by a `(table, AKey)` composite.
#[cfg(any(feature = "entry-timestamps", feature = "permissions"))]
pub(crate) const INODES_TABLE_NAME: &str = "$inodes";

/// The metadata key holding the on-disk format version.
pub(crate) const META_FORMAT_VERSION_KEY: &str = "format_version";

/// The metadata key holding the serialized secondary-index registry.
pub(crate) const META_INDEX_REGISTRY_KEY: &str = "index_registry";

/// The metadata key holding the list of optional on-disk features a reader must
/// support to open this database (for example `permissions`). Features that
/// degrade gracefully — a reader can ignore their data — are never listed here.
pub(crate) const META_REQUIRED_FEATURES_KEY: &str = "requirements";

/// The metadata key holding the serialized user + keyring store (protected DBs).
#[cfg(feature = "permissions")]
pub(crate) const META_USERS_KEY: &str = "users";

/// The metadata key holding the serialized group store (protected DBs).
#[cfg(feature = "permissions")]
pub(crate) const META_GROUPS_KEY: &str = "groups";

/// The metadata key holding the monotonic integrity epoch (protected DBs). Bumped
/// on every change to the user/group store and bound into the control-plane MAC.
#[cfg(feature = "permissions")]
pub(crate) const META_EPOCH_KEY: &str = "epoch";

/// The metadata key holding the control-plane MAC (protected DBs): a keyed BLAKE3
/// tag binding the epoch to the user and group blobs, so tampering with either —
/// for example via a program that opens the redb file directly — is detected at
/// authentication.
#[cfg(feature = "permissions")]
pub(crate) const META_CONTROL_MAC_KEY: &str = "control_mac";

/// The on-disk format version this build reads and writes.
pub(crate) const FORMAT_VERSION: u64 = 1;

/// Whether `name` is a reserved (engine-internal) table name. Every reserved
/// table is `$`-prefixed, so a user table simply may not start with `$`.
pub(crate) fn is_reserved_table_name(name: &str) -> bool {
    name.starts_with('$')
}
