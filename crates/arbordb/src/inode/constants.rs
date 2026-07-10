//! The reserved `$inodes` table handle and the section tags that identify each
//! kind of per-vnode metadata inside an inode blob.

use crate::constants::INODES_TABLE_NAME;
use redb::TableDefinition;

/// The reserved per-vnode metadata table: composite `(table, AKey)` key → a
/// section-tagged blob.
pub(crate) const INODES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new(INODES_TABLE_NAME);

/// The timestamps section: three `i64` Unix-epoch-millisecond fields.
pub(super) const SECTION_TIMESTAMPS: u8 = 0;

/// The ACL section (`permissions` feature): owner `u32`, group `u32`, mode `u16`.
#[cfg(feature = "permissions")]
pub(super) const SECTION_ACL: u8 = 1;

/// The integrity section (`permissions` feature): a 32-byte keyed BLAKE3 tag over
/// the vnode's entry blob and its ACL, verified on authenticated reads.
#[cfg(feature = "permissions")]
pub(super) const SECTION_MAC: u8 = 2;
