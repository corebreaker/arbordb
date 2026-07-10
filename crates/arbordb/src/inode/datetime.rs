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
    pub fn new(created: DateTime<Utc>, modified: DateTime<Utc>, accessed: DateTime<Utc>) -> Self {
        Self {
            created,
            modified,
            accessed,
        }
    }

    pub fn created(&self) -> DateTime<Utc> {
        self.created
    }

    pub fn set_created(&mut self, created: DateTime<Utc>) {
        self.created = created;
    }

    pub fn modified(&self) -> DateTime<Utc> {
        self.modified
    }

    pub fn set_modified(&mut self, modified: DateTime<Utc>) {
        self.modified = modified;
    }

    pub fn accessed(&self) -> DateTime<Utc> {
        self.accessed
    }

    pub fn set_accessed(&mut self, accessed: DateTime<Utc>) {
        self.accessed = accessed;
    }
}
