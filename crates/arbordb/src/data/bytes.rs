//! The [`Bytes`] newtype: an opaque byte string stored as a single leaf.
//!
//! Unlike `Vec<u8>` (which is `AData` as a list of `u8` leaves), `Bytes` maps to
//! one [`Scalar::Bytes`] leaf, so it is both cheaper and read back as a whole.

use super::{
    leaf::{Leaf, LeafMut},
    AData,
    AValue,
    NodeEncoder,
    Scalar,
};

use crate::{
    access::{Reader, Writer},
    error::{AdbError, AdbResult},
    path::VPath,
};

/// An opaque byte string, stored as a single leaf.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl AValue for Bytes {
    fn to_scalar(&self) -> Scalar {
        Scalar::Bytes(self.0.clone())
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        match scalar {
            Scalar::Bytes(bytes) => Ok(Bytes(bytes.clone())),
            other => Err(AdbError::TypeMismatch {
                expected: "bytes",
                found:    other.type_str(),
            }),
        }
    }
}

impl AData for Bytes {
    type Mut<'t> = LeafMut<'t, Bytes>;
    type Ref<'t> = Leaf<'t, Bytes>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        writer.put_scalar(at, self.to_scalar())
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        match reader.scalar_at(at)? {
            Some(scalar) => Bytes::from_scalar(&scalar),
            None => Err(AdbError::PathNotFound(at.clone())),
        }
    }

    fn encode_node(&self, enc: &mut NodeEncoder) -> AdbResult<u32> {
        Ok(enc.leaf(&self.to_scalar()))
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Bytes(bytes)
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}
