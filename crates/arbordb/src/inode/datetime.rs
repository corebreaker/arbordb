//! [`NodeTimestamps`] — the public, decoded form of a vnode's created / modified /
//! accessed times, handed back by the `entry-timestamps` read API. Its on-disk
//! counterpart is [`UnderlyingTimestamps`](super::underlying_timestamps::UnderlyingTimestamps).

use chrono::{DateTime, Utc};

/// The created / modified / accessed timestamps of a file or directory vnode,
/// available with the `entry-timestamps` feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeTimestamps {
    /// When the vnode was first created.
    created: DateTime<Utc>,

    /// When the vnode's content was last modified.
    modified: DateTime<Utc>,

    /// When the vnode was last read. Updated lazily: reads buffer the access time in
    /// memory, and it is persisted on the next committed write or by
    /// [`ArborDb::flush_access_times`](crate::ArborDb::flush_access_times).
    accessed: DateTime<Utc>,
}

impl NodeTimestamps {
    /// Builds a timestamp triple from its `created`, `modified` and `accessed` parts.
    pub fn new(created: DateTime<Utc>, modified: DateTime<Utc>, accessed: DateTime<Utc>) -> Self {
        Self {
            created,
            modified,
            accessed,
        }
    }

    /// When the vnode was first created.
    pub fn created(&self) -> DateTime<Utc> {
        self.created
    }

    /// Overrides the creation time.
    pub fn set_created(&mut self, created: DateTime<Utc>) {
        self.created = created;
    }

    /// When the vnode's content was last modified.
    pub fn modified(&self) -> DateTime<Utc> {
        self.modified
    }

    /// Overrides the last-modified time.
    pub fn set_modified(&mut self, modified: DateTime<Utc>) {
        self.modified = modified;
    }

    /// When the vnode was last read (see the field docs for the lazy-flush caveat).
    pub fn accessed(&self) -> DateTime<Utc> {
        self.accessed
    }

    /// Overrides the last-accessed time.
    pub fn set_accessed(&mut self, accessed: DateTime<Utc>) {
        self.accessed = accessed;
    }
}
