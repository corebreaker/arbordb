pub(crate) fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Writes a length-prefixed (u32 big-endian) byte run.
pub(crate) fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    crate::codec::put_u32(buf, bytes.len() as u32);
    buf.extend_from_slice(bytes);
}
