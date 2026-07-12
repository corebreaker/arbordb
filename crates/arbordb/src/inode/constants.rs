//! The reserved `$inodes` table handle and the section tags that identify each
//! kind of per-a-node metadata inside an inode blob.

use crate::constants::INODES_TABLE_NAME;
use redb::TableDefinition;

/// The reserved per-a-node metadata table: composite `(table, AKey)` key → a
/// section-tagged blob.
pub(crate) const INODES_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new(INODES_TABLE_NAME);

/// The timestamps section: three `i64` Unix-epoch-millisecond fields.
pub(super) const SECTION_TIMESTAMPS: u8 = 0;

/// The ACL section (`permissions` feature): owner `u32`, owner/other grades (one
/// byte each), then a `u32` group count and that many `(gid: u32, grade: u8)` pairs.
#[cfg(feature = "permissions")]
pub(super) const SECTION_ACL: u8 = 1;

/// The integrity section (`permissions` feature): a 32-byte keyed BLAKE3 tag over
/// the a-node's entry blob and its ACL, verified on authenticated reads.
#[cfg(feature = "permissions")]
pub(super) const SECTION_MAC: u8 = 2;

/// The signature section (`permissions` feature): a 64-byte Ed25519 signature over
/// the same bytes the MAC covers (the entry blob and its ACL), verified by a keyless
/// guest — which has the public key but not the private signing key.
#[cfg(feature = "permissions")]
pub(super) const SECTION_SIG: u8 = 3;
