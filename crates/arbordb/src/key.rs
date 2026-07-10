//! The opaque key identifying a file or directory node.

use crate::error::AdbError;
use uuid::Uuid;
use std::{
    cell::Cell,
    fmt::{Debug, Display, Formatter, Result as FmtResult},
    str::FromStr,
};

thread_local! {
    /// splitmix64 state, seeded once per thread from OS entropy. A dedicated fast
    /// PRNG (not a clock + `getrandom` per key like a UUID v7/v4) — minting a key
    /// is a cheap draw with no OS round-trip.
    static KEY_RNG: Cell<u64> = Cell::new(seed());
}

/// A one-time strong per-thread seed. Drawing a single UUID v7 pulls OS entropy
/// (and the current time) once; every subsequent key comes from the cheap PRNG.
fn seed() -> u64 {
    let bits = Uuid::now_v7().as_u128();
    let mixed = (bits as u64) ^ ((bits >> 64) as u64);

    // Guard against an all-zero state (splitmix64 tolerates it, but stay safe).
    mixed | 1
}

/// Opaque key identifying one node — a file or a directory — in the virtual
/// filesystem.
///
/// An [`AKey`] is a node's **stable identity**: it survives renames and moves (a
/// `mv` relinks the name, the key is unchanged). It addresses a whole file or
/// directory, never a [`Scalar`](crate::data::Scalar) or a node *inside* a file's
/// value (that is a [`VPath`](crate::path::VPath)). A fresh key is 128 random bits
/// drawn from a fast per-thread generator (see [`generate`](Self::generate)):
/// unique, fixed 16-byte size, and cheap to mint. Keys are only ever compared for
/// equality and point-looked-up, so no time-ordering is needed. The internal
/// representation is not part of the public API.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AKey(Uuid);

impl AKey {
    /// The fixed key of a table's root directory; path resolution walks from here.
    /// The nil UUID is distinct from any generated key (which is random).
    pub const ROOT: AKey = AKey(Uuid::nil());

    /// One 64-bit split-mix64 draw, advancing `state`.
    fn split_mix64(state: &Cell<u64>) -> u64 {
        let next = state.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        state.set(next);

        let mut z = next;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);

        z ^ (z >> 31)
    }

    /// Generates a fresh, random key from the per-thread fast PRNG.
    pub fn generate() -> Self {
        KEY_RNG.with(|state| {
            let hi = Self::split_mix64(state);
            let lo = Self::split_mix64(state);

            Self(Uuid::from_u128((u128::from(hi) << 64) | u128::from(lo)))
        })
    }

    /// Returns the raw 16-byte representation (big-endian, order-preserving).
    pub fn into_bytes(self) -> [u8; 16] {
        *self.0.as_bytes()
    }

    /// Rebuilds a key from its raw 16-byte representation.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    /// Rebuilds a key from a variable-length byte slice, returning
    /// [`AdbError::BadKey`] if the slice is not exactly 16 bytes long.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, AdbError> {
        if bytes.len() != 16 {
            return Err(AdbError::BadKey(String::from("bytes with length different from 16")));
        }

        let mut array = [0u8; 16];
        array.copy_from_slice(bytes);

        Ok(Self::from_bytes(array))
    }
}

impl From<u128> for AKey {
    fn from(v: u128) -> Self {
        Self(Uuid::from_u128(v))
    }
}

impl From<AKey> for u128 {
    fn from(val: AKey) -> Self {
        val.0.as_u128()
    }
}

impl From<Uuid> for AKey {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<AKey> for Uuid {
    fn from(val: AKey) -> Self {
        val.0
    }
}

impl From<[u8; 16]> for AKey {
    fn from(v: [u8; 16]) -> Self {
        Self::from_bytes(v)
    }
}

impl TryFrom<Vec<u8>> for AKey {
    type Error = AdbError;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from_bytes(&bytes)
    }
}

impl From<AKey> for Vec<u8> {
    fn from(val: AKey) -> Self {
        val.into_bytes().to_vec()
    }
}

impl FromStr for AKey {
    type Err = AdbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|err| AdbError::BadKey(err.to_string()))
    }
}

impl Debug for AKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "AKey({})", self.0)
    }
}

impl Display for AKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "key:{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_yields_distinct_non_root_keys() {
        let a = AKey::generate();
        let b = AKey::generate();

        assert_ne!(a, b);
        assert_ne!(a, AKey::ROOT);
    }

    #[test]
    fn raw_byte_roundtrip() {
        let key = AKey::generate();
        assert_eq!(AKey::from_bytes(key.into_bytes()), key);
        assert_eq!(AKey::from([0u8; 16]), AKey::ROOT);
    }

    #[test]
    fn try_from_bytes_enforces_the_length() {
        let bytes = [7u8; 16];
        assert_eq!(AKey::try_from_bytes(&bytes).unwrap(), AKey::from(bytes));

        let err = AKey::try_from_bytes(&[0u8; 15]).unwrap_err();
        assert!(matches!(err, AdbError::BadKey(_)));

        // The `TryFrom<Vec<u8>>` path shares the same check.
        assert!(AKey::try_from(vec![0u8; 16]).is_ok());
        assert!(AKey::try_from(vec![0u8; 17]).is_err());
    }

    #[test]
    fn integer_and_uuid_conversions_roundtrip() {
        let key = AKey::from(0x0123_4567_89AB_CDEF_u128);
        assert_eq!(u128::from(key), 0x0123_4567_89AB_CDEF);

        let uuid = Uuid::from_u128(42);
        assert_eq!(Uuid::from(AKey::from(uuid)), uuid);

        let bytes: Vec<u8> = AKey::from(0u128).into();
        assert_eq!(bytes, vec![0u8; 16]);
    }

    #[test]
    fn parses_from_a_uuid_string() {
        let uuid = Uuid::from_u128(0xDEAD_BEEF);
        let text = uuid.to_string();

        assert_eq!(AKey::from_str(&text).unwrap(), AKey::from(uuid));
        assert!(matches!(AKey::from_str("not-a-uuid"), Err(AdbError::BadKey(_))));
    }

    #[test]
    fn debug_and_display_render_distinctly() {
        let key = AKey::from(Uuid::nil());

        assert_eq!(format!("{key:?}"), "AKey(00000000-0000-0000-0000-000000000000)");
        assert_eq!(key.to_string(), "key:00000000-0000-0000-0000-000000000000");
    }
}
