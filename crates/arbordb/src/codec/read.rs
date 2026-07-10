//! Bounded random-access reads over a byte blob, shared by the value and
//! directory codecs.

use crate::error::{AdbError, AdbResult};

/// Borrows `len` bytes of `blob` at `off`, or a [`Corrupt`](AdbError::Corrupt)
/// error if that range runs past the end.
pub(crate) fn slice(blob: &[u8], off: usize, len: usize) -> AdbResult<&[u8]> {
    let end = off
        .checked_add(len)
        .ok_or_else(|| AdbError::Corrupt("offset overflow in blob".into()))?;

    blob.get(off..end)
        .ok_or_else(|| AdbError::Corrupt("blob is truncated".into()))
}

/// Reads a single byte at `off`.
pub(crate) fn read_u8(blob: &[u8], off: usize) -> AdbResult<u8> {
    slice(blob, off, 1).map(|bytes| bytes[0])
}

/// Reads a big-endian `u32` at `off`.
pub(crate) fn read_u32(blob: &[u8], off: usize) -> AdbResult<u32> {
    Ok(u32::from_be_bytes(slice(blob, off, 4)?.try_into().unwrap()))
}
