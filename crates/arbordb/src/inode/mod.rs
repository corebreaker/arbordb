//! Per-vnode metadata — created/modified/accessed datetime (and, with the
//! `permissions` feature, ACLs and an integrity tag) — held out-of-band in the
//! reserved `$inodes` table, keyed by `(table name, vnode key)`.
//!
//! Keeping this out of the vnode's own entry blob leaves the data-table format
//! untouched: a binary built without `entry-timestamps` never opens `$inodes`, so
//! it reads and writes a timestamped database and simply ignores the metadata.
//! It also keeps a deferred access-time flush cheap — it rewrites a ~40-byte inode
//! blob, never the (possibly large) value it describes.
//!
//! Each `$inodes` value is a small section-tagged blob so it stays forward- and
//! backward-compatible within the feature matrix: a reader copies through any
//! section tag it does not recognize (for example a `permissions` build's ACL
//! section, seen by an `entry-timestamps`-only build).

#[cfg(feature = "permissions")]
mod acl;
mod constants;
mod datetime;
mod functions;
mod node;
mod table;
mod underlying_timestamps;

pub(crate) use self::{
    constants::INODES_TABLE,
    functions::{bump_access, forget, touch, read_times},
    table::InodeTable,
};

#[cfg(feature = "permissions")]
pub(crate) use self::{
    acl::{mode_to_bits, Acl, Class, Right},
    functions::{read_acl, read_mac, seal_mac, set_acl, set_default_acl, strip_group},
};

pub use datetime::NodeTimestamps;
