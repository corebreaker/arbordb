use crate::error::{AdbError, AdbResult};

/// A forward-only cursor over a byte buffer used during decoding.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            pos: 0,
        }
    }

    fn take(&mut self, n: usize) -> AdbResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| AdbError::Corrupt("length overflow while decoding".into()))?;

        if end > self.buf.len() {
            return Err(AdbError::Corrupt("unexpected end of buffer while decoding".into()));
        }

        let slice = &self.buf[self.pos..end];
        self.pos = end;

        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> AdbResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> AdbResult<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(crate) fn u64(&mut self) -> AdbResult<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// Reads a fixed-size array of `N` bytes.
    pub(crate) fn array<const N: usize>(&mut self) -> AdbResult<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    /// Reads a length-prefixed byte run written by [`super::put_bytes`].
    pub(crate) fn bytes(&mut self) -> AdbResult<&'a [u8]> {
        let n = self.u32()? as usize;

        self.take(n)
    }

    /// The number of bytes consumed so far — the encoded length of what was read.
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// Whether every byte has been consumed (used by round-trip tests).
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.pos >= self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_past_the_end_is_an_error() {
        let mut r = Reader::new(&[1, 2]);

        assert_eq!(r.u8().unwrap(), 1);
        assert!(r.u32().is_err()); // only one byte remains
    }

    #[test]
    fn a_bogus_length_prefix_is_rejected() {
        // `bytes()` reads a u32 length then that many bytes; a huge length overruns.
        let mut r = Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x00]);

        assert!(r.bytes().is_err());
    }
}
