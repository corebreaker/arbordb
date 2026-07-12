//! [`UnderlyingTimestamps`] — the on-disk form of an a-node's three timestamps
//! (Unix-epoch milliseconds), and its conversion to the public
//! [`NodeTimestamps`](super::datetime::NodeTimestamps).

use super::NodeTimestamps;
use crate::{
    codec::Reader,
    error::{AdbError, AdbResult},
};

use chrono::{DateTime, Utc};

/// An a-node's three timestamps in on-disk form (Unix-epoch milliseconds).
#[derive(Clone, Copy)]
pub(super) struct UnderlyingTimestamps {
    /// Creation time, Unix-epoch milliseconds.
    created:  i64,
    /// Last-modified time, Unix-epoch milliseconds.
    modified: i64,
    /// Last-accessed time, Unix-epoch milliseconds.
    accessed: i64,
}

impl UnderlyingTimestamps {
    /// Builds a triple from raw epoch-millisecond fields.
    pub(super) fn new(created: i64, modified: i64, accessed: i64) -> Self {
        Self {
            created,
            modified,
            accessed,
        }
    }

    /// Decodes three big-endian `i64` fields from a timestamps section body.
    pub(super) fn decode(body: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(body);

        Ok(Self {
            created:  r.u64()? as i64,
            modified: r.u64()? as i64,
            accessed: r.u64()? as i64,
        })
    }

    /// Encodes the three fields as consecutive big-endian `i64`s.
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&self.created.to_be_bytes());
        out.extend_from_slice(&self.modified.to_be_bytes());
        out.extend_from_slice(&self.accessed.to_be_bytes());

        out
    }

    /// The raw creation time.
    pub(super) fn created(&self) -> i64 {
        self.created
    }

    /// The raw last-accessed time.
    pub(super) fn accessed(&self) -> i64 {
        self.accessed
    }

    /// Overrides the raw last-accessed time.
    pub(super) fn set_accessed(&mut self, accessed: i64) {
        self.accessed = accessed;
    }

    /// Converts to the public [`NodeTimestamps`], rejecting any out-of-range
    /// millisecond value as corrupt.
    pub(super) fn into_node_times(self) -> AdbResult<NodeTimestamps> {
        let to_dt = |millis: i64| {
            DateTime::<Utc>::from_timestamp_millis(millis)
                .ok_or_else(|| AdbError::Corrupt("out-of-range inode timestamp".into()))
        };

        Ok(NodeTimestamps::new(
            to_dt(self.created)?,
            to_dt(self.modified)?,
            to_dt(self.accessed)?,
        ))
    }
}
