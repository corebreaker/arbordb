//! Date/time (de)serialization helpers for the temporal scalar variants.

use crate::{
    codec::Reader,
    error::{AdbError, AdbResult},
};

use chrono::NaiveTime;

/// Decodes a [`NaiveTime`] from its seconds-since-midnight and nanoseconds encoding.
pub(crate) fn decode_time(r: &mut Reader<'_>) -> AdbResult<NaiveTime> {
    let secs = r.u32()?;
    let nanos = r.u32()?;

    NaiveTime::from_num_seconds_from_midnight_opt(secs, nanos)
        .ok_or_else(|| AdbError::Corrupt("out-of-range time scalar".into()))
}

/// The current wall-clock time as Unix-epoch milliseconds — the on-disk unit.
#[cfg(feature = "entry-timestamps")]
pub(crate) fn timestamp_now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
