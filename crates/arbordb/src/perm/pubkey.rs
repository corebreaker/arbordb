//! The public verification key of a protected database — the "read key" a keyless
//! guest uses to check each value's signature.
//!
//! It is public by nature, so it can be saved and shared freely: retrieved with
//! [`ArborDb::pubkey`](crate::ArborDb::pubkey), moved around as raw bytes
//! ([`as_bytes`](PublicKey::as_bytes) / [`into_bytes`](PublicKey::into_bytes) /
//! [`from_bytes`](PublicKey::from_bytes)), read from or written to a file
//! ([`read_key`](PublicKey::read_key) / [`write_key`](PublicKey::write_key)), stored
//! as an ArborDb value (it implements [`AValue`](crate::data::AValue) and
//! [`AData`](crate::data::AData)), or serialized with Serde (behind the `serde`
//! feature).
//!
//! # Guest reads and trust
//!
//! A keyless guest reads without the integrity key, so on its own it verifies each
//! value's signature against the public key **stored in the database file**. That
//! guards against accidental corruption and naive tampering, but not against an
//! attacker who edits the file to swap the stored key *and* re-sign — so **a default
//! guest read is not trustworthy**. To read as a guest safely, obtain this key while
//! the database is trusted, keep it out-of-band, and pin it with
//! [`ArborDb::with_pubkey`](crate::ArborDb::with_pubkey); verification then uses
//! the pinned key, which a swap of the stored key cannot defeat. Pinning is optional:
//! a database that never exports its key is still fully write-protected (a guest
//! cannot write), but its guest reads should be treated as untrusted.

use crate::{
    access::{Reader, Writer},
    crypto::PUBKEY_LEN,
    data::{AData, AValue, Leaf, LeafMut, Scalar},
    error::{AdbError, AdbResult},
    path::VPath,
};

use std::path::Path;

/// The Ed25519 public verification key of a protected database.
///
/// See [`ArborDb::with_pubkey`](crate::ArborDb::with_pubkey) for how a guest uses it
/// and why pinning a trusted copy matters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(transparent))]
pub struct PublicKey {
    /// The raw 32-byte Ed25519 verification key.
    bytes: [u8; PUBKEY_LEN],
}

impl PublicKey {
    /// Wraps raw key bytes.
    pub fn from_bytes(bytes: [u8; PUBKEY_LEN]) -> Self {
        Self {
            bytes,
        }
    }

    /// Borrows the raw key bytes.
    pub fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.bytes
    }

    /// Consumes the key, returning its raw bytes.
    pub fn into_bytes(self) -> [u8; PUBKEY_LEN] {
        self.bytes
    }

    /// Reads a key previously saved by [`write_key`](Self::write_key). Errors if the
    /// file cannot be read or does not hold exactly one key.
    pub fn read_key(path: impl AsRef<Path>) -> AdbResult<Self> {
        let bytes = std::fs::read(path.as_ref())
            .map_err(|e| AdbError::CannotAccess(format!("reading the public key failed: {e}")))?;

        let bytes: [u8; PUBKEY_LEN] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| AdbError::Corrupt(String::from("the public key file has the wrong length")))?;

        Ok(Self {
            bytes,
        })
    }

    /// Writes the raw key bytes to `path`, truncating any existing file.
    pub fn write_key(&self, path: impl AsRef<Path>) -> AdbResult<()> {
        std::fs::write(path.as_ref(), self.bytes)
            .map_err(|e| AdbError::CannotAccess(format!("writing the public key failed: {e}")))
    }
}

impl AValue for PublicKey {
    fn to_scalar(&self) -> Scalar {
        Scalar::Bytes(self.bytes.to_vec())
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        match scalar {
            Scalar::Bytes(bytes) => {
                let bytes: [u8; PUBKEY_LEN] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| AdbError::Corrupt(String::from("a public key must be exactly 32 bytes")))?;

                Ok(Self {
                    bytes,
                })
            }
            other => Err(AdbError::TypeMismatch {
                expected: "pubkey",
                found:    other.type_str(),
            }),
        }
    }
}

impl AData for PublicKey {
    type Mut<'t> = LeafMut<'t, PublicKey>;
    type Ref<'t> = Leaf<'t, PublicKey>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        writer.put_scalar(at, self.to_scalar())
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        match reader.scalar_at(at)? {
            Some(scalar) => Self::from_scalar(&scalar),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }
}
