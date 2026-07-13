//! [`AData`] for the big-number types when they are stored as composite data
//! rather than as a native [`Scalar`] variant.
//!
//! These impls are compiled only for a `*-as-data` feature whose matching
//! `*-as-scalar` feature is **off**: in that configuration the type is not a
//! `Scalar`, so it cannot persist as a single native leaf. Instead each value is
//! serialised to bytes and stored as one [`Bytes`] leaf, and the read/write
//! accessors are those of `Bytes` (`get()` yields the raw `Bytes`; recompose the
//! typed value with `ReadTxn::load::<T>`).

use super::{
    AData,
    Bytes,
    Scalar,
    leaf::{Leaf, LeafMut},
};

use crate::{
    access::{Reader, Writer},
    error::{AdbError, AdbResult},
    path::VPath,
};

#[cfg(all(not(feature = "bigint-as-scalar"), feature = "bigint-as-data"))]
use num_bigint::BigInt;

#[cfg(all(not(feature = "bigfloat-as-scalar"), feature = "bigfloat-as-data"))]
use num_bigfloat::{BigFloat, INF_NEG, INF_POS, NAN, ZERO};

#[cfg(all(not(feature = "rational-as-scalar"), feature = "rational-as-data"))]
use super::rational::BigRational;

/// Reads the single `Bytes` leaf a big-number value was stored as.
fn load_leaf_bytes<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Vec<u8>> {
    match reader.scalar_at(at)? {
        Some(Scalar::Bytes(bytes)) => Ok(bytes),
        Some(other) => Err(AdbError::TypeMismatch {
            expected: "bignum-bytes",
            found:    other.type_str(),
        }),
        None => Err(AdbError::PathNotFound(at.clone())),
    }
}

#[cfg(all(not(feature = "bigint-as-scalar"), feature = "bigint-as-data"))]
impl AData for BigInt {
    type Mut<'t> = LeafMut<'t, Bytes>;
    type Ref<'t> = Leaf<'t, Bytes>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        writer.put_scalar(at, Scalar::Bytes(self.to_signed_bytes_be()))
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        Ok(BigInt::from_signed_bytes_be(&load_leaf_bytes(reader, at)?))
    }
}

#[cfg(all(not(feature = "rational-as-scalar"), feature = "rational-as-data"))]
impl AData for BigRational {
    type Mut<'t> = LeafMut<'t, Bytes>;
    type Ref<'t> = Leaf<'t, Bytes>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        let mut bytes = Vec::new();
        crate::codec::put_bytes(&mut bytes, &self.numer().to_signed_bytes_be());
        crate::codec::put_bytes(&mut bytes, &self.denom().to_signed_bytes_be());

        writer.put_scalar(at, Scalar::Bytes(bytes))
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        let bytes = load_leaf_bytes(reader, at)?;
        let mut reader = crate::codec::Reader::new(&bytes);

        let numer = num_bigint::BigInt::from_signed_bytes_be(reader.bytes()?);
        let denom = num_bigint::BigInt::from_signed_bytes_be(reader.bytes()?);

        Ok(BigRational::new(numer, denom))
    }
}

#[cfg(all(not(feature = "bigfloat-as-scalar"), feature = "bigfloat-as-data"))]
impl AData for BigFloat {
    type Mut<'t> = LeafMut<'t, Bytes>;
    type Ref<'t> = Leaf<'t, Bytes>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        writer.put_scalar(at, Scalar::Bytes(bigfloat::to_bytes(self)))
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        bigfloat::from_bytes(&load_leaf_bytes(reader, at)?)
    }
}

/// Self-contained byte encoding for a [`BigFloat`] stored as data.
///
/// A leading tag byte distinguishes the special values; a finite, non-zero number
/// is `[sign, exponent, mantissa…]` where `exponent` is the raw `i8` reinterpreted
/// as a byte and `mantissa` is the remainder of the buffer (one decimal digit per
/// byte, most significant first).
#[cfg(all(not(feature = "bigfloat-as-scalar"), feature = "bigfloat-as-data"))]
mod bigfloat {
    use super::{BigFloat, INF_NEG, INF_POS, NAN, ZERO};
    use crate::error::{AdbError, AdbResult};

    const NAN_TAG: u8 = 0;
    const INF_POS_TAG: u8 = 1;
    const INF_NEG_TAG: u8 = 2;
    const ZERO_TAG: u8 = 3;
    const NEG_TAG: u8 = 4;
    const POS_TAG: u8 = 5;

    pub(super) fn to_bytes(v: &BigFloat) -> Vec<u8> {
        let mut out = Vec::new();

        // Same ordered checks as the scalar encoding: NaN and the infinities must be
        // caught before the sign/zero tests, which do not describe them.
        if v.is_nan() {
            out.push(NAN_TAG);
        } else if v.is_inf_pos() {
            out.push(INF_POS_TAG);
        } else if v.is_inf_neg() {
            out.push(INF_NEG_TAG);
        } else if v.is_zero() {
            out.push(ZERO_TAG);
        } else {
            out.push(if v.is_negative() { NEG_TAG } else { POS_TAG });
            out.push(v.get_exponent() as u8);

            let mut mantissa = vec![0u8; v.get_mantissa_len()];
            v.get_mantissa_bytes(&mut mantissa);
            out.extend_from_slice(&mantissa);
        }

        out
    }

    pub(super) fn from_bytes(bytes: &[u8]) -> AdbResult<BigFloat> {
        let corrupt = || AdbError::Corrupt("invalid bigfloat encoding".into());
        let (&tag, rest) = bytes.split_first().ok_or_else(corrupt)?;

        let value = match tag {
            NAN_TAG => NAN,
            INF_POS_TAG => INF_POS,
            INF_NEG_TAG => INF_NEG,
            ZERO_TAG => ZERO,
            NEG_TAG | POS_TAG => {
                let (&exponent, mantissa) = rest.split_first().ok_or_else(corrupt)?;
                let sign = if tag == NEG_TAG { -1 } else { 1 };

                BigFloat::from_bytes(mantissa, sign, exponent as i8)
            }
            _ => return Err(corrupt()),
        };

        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArborDb;

    fn store_then_load<T: AData + PartialEq + std::fmt::Debug>(value: T) {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        {
            let w = table.write().unwrap();
            w.store::<T>("x", &value).unwrap();
            w.commit().unwrap();
        }

        let r = table.read().unwrap();
        assert_eq!(r.load::<T>("x").unwrap(), Some(value));
    }

    #[cfg(all(not(feature = "bigint-as-scalar"), feature = "bigint-as-data"))]
    #[test]
    fn bigint_roundtrips_as_data() {
        let big = BigInt::parse_bytes(b"123456789012345678901234567890", 10).unwrap();

        store_then_load(BigInt::from(0));
        store_then_load(BigInt::from(128));
        store_then_load(BigInt::from(-129));
        store_then_load(big.clone());
        store_then_load(-big);
    }

    #[cfg(all(not(feature = "rational-as-scalar"), feature = "rational-as-data"))]
    #[test]
    fn rational_roundtrips_as_data() {
        store_then_load(BigRational::new(
            num_bigint::BigInt::from(0),
            num_bigint::BigInt::from(1),
        ));

        store_then_load(BigRational::new(
            num_bigint::BigInt::from(1),
            num_bigint::BigInt::from(3),
        ));

        store_then_load(BigRational::new(
            num_bigint::BigInt::from(-7),
            num_bigint::BigInt::from(2),
        ));
    }

    #[cfg(all(not(feature = "bigfloat-as-scalar"), feature = "bigfloat-as-data"))]
    #[test]
    fn bigfloat_roundtrips_as_data() {
        store_then_load(ZERO);
        store_then_load(BigFloat::from_f64(123.42));
        store_then_load(BigFloat::from_f64(-123.42));
        store_then_load(INF_POS);
        store_then_load(INF_NEG);

        // NaN never equals itself, so check the decoded flavour explicitly.
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        {
            let w = table.write().unwrap();
            w.store::<BigFloat>("n", &NAN).unwrap();
            w.commit().unwrap();
        }

        let r = table.read().unwrap();
        assert!(r.load::<BigFloat>("n").unwrap().unwrap().is_nan());
    }
}
