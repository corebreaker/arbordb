use super::NodeTimestamps;
use crate::{
    codec::Reader,
    error::{AdbError, AdbResult},
};

use chrono::{DateTime, Utc};

/// A vnode's three timestamps in on-disk form (Unix-epoch milliseconds).
#[derive(Clone, Copy)]
pub(super) struct UnderlyingTimestamps {
    created:  i64,
    modified: i64,
    accessed: i64,
}

impl UnderlyingTimestamps {
    pub(super) fn new(created: i64, modified: i64, accessed: i64) -> Self {
        Self {
            created,
            modified,
            accessed,
        }
    }

    pub(super) fn decode(body: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(body);

        Ok(Self {
            created:  r.u64()? as i64,
            modified: r.u64()? as i64,
            accessed: r.u64()? as i64,
        })
    }

    pub(super) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        out.extend_from_slice(&self.created.to_be_bytes());
        out.extend_from_slice(&self.modified.to_be_bytes());
        out.extend_from_slice(&self.accessed.to_be_bytes());

        out
    }

    pub(super) fn created(&self) -> i64 {
        self.created
    }

    pub(super) fn accessed(&self) -> i64 {
        self.accessed
    }

    pub(super) fn set_accessed(&mut self, accessed: i64) {
        self.accessed = accessed;
    }

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
