//! Small decoding helpers shared across the index-definition codecs.

use crate::{AdbError, AdbResult, codec::Reader};

/// Reads a length-prefixed UTF-8 string, rejecting invalid UTF-8 as corrupt.
pub(super) fn read_string(r: &mut Reader<'_>) -> AdbResult<String> {
    std::str::from_utf8(r.bytes()?)
        .map(str::to_string)
        .map_err(|_| AdbError::Corrupt("invalid utf-8 in index definition".into()))
}
