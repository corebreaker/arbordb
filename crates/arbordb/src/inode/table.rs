//! [`InodeTable`] — the writable-handle type alias for the reserved `$inodes` table.

use redb::Table;

/// A writable handle to the per-a-node metadata table.
pub(crate) type InodeTable<'txn> = Table<'txn, &'static [u8], &'static [u8]>;
