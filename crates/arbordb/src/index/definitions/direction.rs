//! [`Direction`] — the ascending/descending sort order of an index column, plus
//! its one-byte on-disk encoding.

use crate::error::{AdbError, AdbResult};

/// The sort direction of an index column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// Ascending order.
    Asc,
    /// Descending order.
    Desc,
}

impl Direction {
    /// The one-byte encoding stored in an index definition (`Asc` → 0, `Desc` → 1).
    pub(super) fn to_byte(self) -> u8 {
        match self {
            Direction::Asc => 0,
            Direction::Desc => 1,
        }
    }

    /// Decodes a byte written by [`to_byte`](Self::to_byte); any other value is corrupt.
    pub(super) fn from_byte(byte: u8) -> AdbResult<Self> {
        match byte {
            0 => Ok(Direction::Asc),
            1 => Ok(Direction::Desc),
            other => Err(AdbError::Corrupt(format!("unknown index direction {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_roundtrip_and_rejects_unknown() {
        assert_eq!(Direction::from_byte(Direction::Asc.to_byte()).unwrap(), Direction::Asc);
        assert_eq!(
            Direction::from_byte(Direction::Desc.to_byte()).unwrap(),
            Direction::Desc
        );
        assert!(matches!(Direction::from_byte(9), Err(AdbError::Corrupt(_))));
    }
}
