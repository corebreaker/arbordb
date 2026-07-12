//! Scalar leaf values: the [`Scalar`] enum and its storage encoding.

use crate::{
    codec::{self, Reader},
    time::decode_time,
    error::{AdbError, AdbResult},
};

use uuid::Uuid;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike, Utc};

#[cfg(any(feature = "bigint-as-scalar", feature = "rational-as-scalar"))]
use num_bigint::BigInt;

#[cfg(feature = "bigfloat-as-scalar")]
use num_bigfloat::{BigFloat, INF_NEG, INF_POS, NAN as FLOAT_NAN, ZERO as FLOAT_ZERO};

#[cfg(feature = "rational-as-scalar")]
use num_rational::BigRational;

mod tag {
    pub(super) const NULL: u8 = 0;
    pub(super) const BOOL: u8 = 1;
    pub(super) const I8: u8 = 2;
    pub(super) const I16: u8 = 3;
    pub(super) const I32: u8 = 4;
    pub(super) const I64: u8 = 5;
    pub(super) const I128: u8 = 6;
    pub(super) const U8: u8 = 7;
    pub(super) const U16: u8 = 8;
    pub(super) const U32: u8 = 9;
    pub(super) const U64: u8 = 10;
    pub(super) const U128: u8 = 11;
    pub(super) const F32: u8 = 12;
    pub(super) const F64: u8 = 13;
    pub(super) const STR: u8 = 14;
    pub(super) const BYTES: u8 = 15;
    pub(super) const UUID: u8 = 16;
    pub(super) const DATE: u8 = 17;
    pub(super) const TIME: u8 = 18;
    pub(super) const DATETIME: u8 = 19;
    pub(super) const DURATION: u8 = 20;
    #[cfg(feature = "bigint-as-scalar")]
    pub(super) const BIG_INT: u8 = 21;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_ZERO: u8 = 22;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_POS: u8 = 23;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_NEG: u8 = 24;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_NAN: u8 = 25;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_INF_POS: u8 = 26;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_INF_NEG: u8 = 27;
    #[cfg(feature = "rational-as-scalar")]
    pub(super) const BIG_RATIONAL: u8 = 28;
}

/// A persisted scalar value: the content of a leaf vnode.
///
/// This is the dynamic, runtime representation of any value ArborDb can store in
/// a leaf. Rust types map to and from it through the [`AValue`](super::AValue) trait.
#[derive(Default, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Scalar {
    /// The absence of a value.
    #[default]
    Null,

    /// A boolean.
    Bool(bool),

    /// A signed 8-bit integer.
    I8(i8),

    /// A signed 16-bit integer.
    I16(i16),

    /// A signed 32-bit integer.
    I32(i32),

    /// A signed 64-bit integer.
    I64(i64),

    /// A signed 128-bit integer.
    I128(i128),

    /// An unsigned 8-bit integer.
    U8(u8),

    /// An unsigned 16-bit integer.
    U16(u16),

    /// An unsigned 32-bit integer.
    U32(u32),

    /// An unsigned 64-bit integer.
    U64(u64),

    /// An unsigned 128-bit integer.
    U128(u128),

    /// A 32-bit floating point number.
    F32(f32),

    /// A 64-bit floating point number.
    F64(f64),

    /// A UTF-8 string.
    Str(String),

    /// An opaque byte string.
    Bytes(Vec<u8>),

    /// A free-standing UUID value (not a vnode key).
    Uuid(Uuid),

    /// A calendar date with no time zone.
    Date(NaiveDate),

    /// A wall-clock time with no time zone.
    Time(NaiveTime),

    /// A date and time with no time zone.
    DateTime(DateTime<Utc>),

    /// A signed duration.
    Duration(TimeDelta),

    /// An arbitrary-precision signed integer.
    #[cfg(feature = "bigint-as-scalar")]
    BigInt(BigInt),

    /// A fixed-precision decimal float.
    #[cfg(feature = "bigfloat-as-scalar")]
    BigFloat(BigFloat),

    /// An arbitrary-precision rational.
    #[cfg(feature = "rational-as-scalar")]
    Rational(BigRational),
}

impl Scalar {
    /// A short, stable label for the variant, used in diagnostics.
    pub(crate) fn type_str(&self) -> &'static str {
        match self {
            Scalar::Null => "null",
            Scalar::Bool(_) => "bool",
            Scalar::I8(_) => "i8",
            Scalar::I16(_) => "i16",
            Scalar::I32(_) => "i32",
            Scalar::I64(_) => "i64",
            Scalar::I128(_) => "i128",
            Scalar::U8(_) => "u8",
            Scalar::U16(_) => "u16",
            Scalar::U32(_) => "u32",
            Scalar::U64(_) => "u64",
            Scalar::U128(_) => "u128",
            Scalar::F32(_) => "f32",
            Scalar::F64(_) => "f64",
            Scalar::Str(_) => "str",
            Scalar::Bytes(_) => "bytes",
            Scalar::Uuid(_) => "uuid",
            Scalar::Date(_) => "date",
            Scalar::Time(_) => "time",
            Scalar::DateTime(_) => "datetime",
            Scalar::Duration(_) => "duration",
            #[cfg(feature = "bigint-as-scalar")]
            Scalar::BigInt(_) => "bigint",
            #[cfg(feature = "bigfloat-as-scalar")]
            Scalar::BigFloat(_) => "bigfloat",
            #[cfg(feature = "rational-as-scalar")]
            Scalar::Rational(_) => "rational",
        }
    }

    /// Appends the exact (round-trippable) storage encoding of this scalar.
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Scalar::Null => buf.push(tag::NULL),
            Scalar::Bool(v) => {
                buf.push(tag::BOOL);
                buf.push(u8::from(*v));
            }
            Scalar::I8(v) => {
                buf.push(tag::I8);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::I16(v) => {
                buf.push(tag::I16);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::I32(v) => {
                buf.push(tag::I32);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::I64(v) => {
                buf.push(tag::I64);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::I128(v) => {
                buf.push(tag::I128);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::U8(v) => {
                buf.push(tag::U8);
                buf.push(*v);
            }
            Scalar::U16(v) => {
                buf.push(tag::U16);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::U32(v) => {
                buf.push(tag::U32);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::U64(v) => {
                buf.push(tag::U64);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::U128(v) => {
                buf.push(tag::U128);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::F32(v) => {
                buf.push(tag::F32);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::F64(v) => {
                buf.push(tag::F64);
                buf.extend_from_slice(&v.to_be_bytes());
            }
            Scalar::Str(v) => {
                buf.push(tag::STR);
                codec::put_bytes(buf, v.as_bytes());
            }
            Scalar::Bytes(v) => {
                buf.push(tag::BYTES);
                codec::put_bytes(buf, v);
            }
            Scalar::Uuid(v) => {
                buf.push(tag::UUID);
                buf.extend_from_slice(v.as_bytes());
            }
            Scalar::Date(v) => {
                buf.push(tag::DATE);
                buf.extend_from_slice(&v.num_days_from_ce().to_be_bytes());
            }
            Scalar::Time(v) => {
                buf.push(tag::TIME);
                buf.extend_from_slice(&v.num_seconds_from_midnight().to_be_bytes());
                buf.extend_from_slice(&v.nanosecond().to_be_bytes());
            }
            Scalar::DateTime(v) => {
                buf.push(tag::DATETIME);
                buf.extend_from_slice(&v.naive_utc().num_days_from_ce().to_be_bytes());
                buf.extend_from_slice(&v.time().num_seconds_from_midnight().to_be_bytes());
                buf.extend_from_slice(&v.time().nanosecond().to_be_bytes());
            }
            Scalar::Duration(v) => {
                buf.push(tag::DURATION);
                buf.extend_from_slice(&v.num_seconds().to_be_bytes());
                buf.extend_from_slice(&v.subsec_nanos().to_be_bytes());
            }
            #[cfg(feature = "bigint-as-scalar")]
            Scalar::BigInt(v) => {
                buf.push(tag::BIG_INT);
                codec::put_bytes(buf, &v.to_signed_bytes_be());
            }
            #[cfg(feature = "bigfloat-as-scalar")]
            Scalar::BigFloat(v) => {
                encode_float(buf, v);
            }
            #[cfg(feature = "rational-as-scalar")]
            Scalar::Rational(v) => {
                buf.push(tag::BIG_RATIONAL);
                codec::put_bytes(buf, &v.numer().to_signed_bytes_be());
                codec::put_bytes(buf, &v.denom().to_signed_bytes_be());
            }
        }
    }

    /// Decodes a scalar previously written by [`Scalar::encode`].
    pub(crate) fn decode(r: &mut Reader<'_>) -> AdbResult<Scalar> {
        let tag = r.u8()?;
        let scalar = match tag {
            tag::NULL => Scalar::Null,
            tag::BOOL => Scalar::Bool(r.u8()? != 0),
            tag::I8 => Scalar::I8(i8::from_be_bytes(r.array()?)),
            tag::I16 => Scalar::I16(i16::from_be_bytes(r.array()?)),
            tag::I32 => Scalar::I32(i32::from_be_bytes(r.array()?)),
            tag::I64 => Scalar::I64(i64::from_be_bytes(r.array()?)),
            tag::I128 => Scalar::I128(i128::from_be_bytes(r.array()?)),
            tag::U8 => Scalar::U8(r.u8()?),
            tag::U16 => Scalar::U16(u16::from_be_bytes(r.array()?)),
            tag::U32 => Scalar::U32(r.u32()?),
            tag::U64 => Scalar::U64(r.u64()?),
            tag::U128 => Scalar::U128(u128::from_be_bytes(r.array()?)),
            tag::F32 => Scalar::F32(f32::from_be_bytes(r.array()?)),
            tag::F64 => Scalar::F64(f64::from_be_bytes(r.array()?)),
            tag::STR => {
                let bytes = r.bytes()?;
                let s = std::str::from_utf8(bytes)
                    .map_err(|_| AdbError::Corrupt("invalid utf-8 in string scalar".into()))?;

                Scalar::Str(s.to_string())
            }
            tag::BYTES => Scalar::Bytes(r.bytes()?.to_vec()),
            tag::UUID => Scalar::Uuid(Uuid::from_bytes(r.array()?)),
            tag::DATE => {
                let days = i32::from_be_bytes(r.array()?);
                let date = NaiveDate::from_num_days_from_ce_opt(days)
                    .ok_or_else(|| AdbError::Corrupt("out-of-range date scalar".into()))?;

                Scalar::Date(date)
            }
            tag::TIME => Scalar::Time(decode_time(r)?),
            tag::DATETIME => {
                let days = i32::from_be_bytes(r.array()?);
                let date = NaiveDate::from_num_days_from_ce_opt(days)
                    .ok_or_else(|| AdbError::Corrupt("out-of-range datetime scalar".into()))?;
                let time = decode_time(r)?;

                Scalar::DateTime(NaiveDateTime::new(date, time).and_utc())
            }
            tag::DURATION => {
                let secs = i64::from_be_bytes(r.array()?);
                let nanos = i32::from_be_bytes(r.array()?);

                Scalar::Duration(TimeDelta::seconds(secs) + TimeDelta::nanoseconds(i64::from(nanos)))
            }
            #[cfg(feature = "bigint-as-scalar")]
            tag::BIG_INT => Scalar::BigInt(BigInt::from_signed_bytes_be(r.bytes()?)),
            #[cfg(feature = "bigfloat-as-scalar")]
            tag::BIG_FLOAT_ZERO => Scalar::BigFloat(FLOAT_ZERO),
            #[cfg(feature = "bigfloat-as-scalar")]
            tag::BIG_FLOAT_NAN => Scalar::BigFloat(FLOAT_NAN),
            #[cfg(feature = "bigfloat-as-scalar")]
            tag::BIG_FLOAT_INF_POS => Scalar::BigFloat(INF_POS),
            #[cfg(feature = "bigfloat-as-scalar")]
            tag::BIG_FLOAT_INF_NEG => Scalar::BigFloat(INF_NEG),
            #[cfg(feature = "bigfloat-as-scalar")]
            tag::BIG_FLOAT_POS => {
                let exponent = i8::from_be_bytes(r.array()?);
                let mantissa = r.bytes()?;

                Scalar::BigFloat(BigFloat::from_bytes(mantissa, 1, exponent))
            }
            #[cfg(feature = "bigfloat-as-scalar")]
            tag::BIG_FLOAT_NEG => {
                let exponent = i8::from_be_bytes(r.array()?);
                let mantissa = r.bytes()?;

                Scalar::BigFloat(BigFloat::from_bytes(mantissa, -1, exponent))
            }
            #[cfg(feature = "rational-as-scalar")]
            tag::BIG_RATIONAL => {
                let num = BigInt::from_signed_bytes_be(r.bytes()?);
                let den = BigInt::from_signed_bytes_be(r.bytes()?);

                Scalar::Rational(BigRational::new(num, den))
            }
            other => return Err(AdbError::Corrupt(format!("unknown scalar tag {other}"))),
        };

        Ok(scalar)
    }
}

#[cfg(feature = "bigfloat-as-scalar")]
fn encode_float(buf: &mut Vec<u8>, v: &BigFloat) {
    // The special values each get their own tag; only finite, non-zero numbers
    // carry a sign/exponent/mantissa body. The order of the checks matters:
    // `is_negative`/`is_zero` are false for NaN, and a sign test alone cannot
    // distinguish the two infinities.
    if v.is_nan() {
        buf.push(tag::BIG_FLOAT_NAN);
        return;
    }

    if v.is_inf_pos() {
        buf.push(tag::BIG_FLOAT_INF_POS);
        return;
    }

    if v.is_inf_neg() {
        buf.push(tag::BIG_FLOAT_INF_NEG);
        return;
    }

    if v.is_zero() {
        buf.push(tag::BIG_FLOAT_ZERO);
        return;
    }

    let sign = if v.is_negative() {
        tag::BIG_FLOAT_NEG
    } else {
        tag::BIG_FLOAT_POS
    };

    let mantissa = {
        let mut mantissa = vec![0u8; v.get_mantissa_len()];
        v.get_mantissa_bytes(&mut mantissa);
        mantissa
    };

    buf.push(sign);
    buf.extend_from_slice(&v.get_exponent().to_be_bytes());
    codec::put_bytes(buf, &mantissa);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(scalar: Scalar) {
        let mut buf = Vec::new();
        scalar.encode(&mut buf);

        let mut reader = Reader::new(&buf);
        let decoded = Scalar::decode(&mut reader).expect("decode");

        assert!(reader.is_empty(), "trailing bytes for {scalar:?}");
        assert_eq!(scalar, decoded);
    }

    #[test]
    fn scalar_roundtrips() {
        roundtrip(Scalar::Null);
        roundtrip(Scalar::Bool(true));
        roundtrip(Scalar::Bool(false));
        roundtrip(Scalar::I8(-8));
        roundtrip(Scalar::I16(-1600));
        roundtrip(Scalar::I32(-42));
        roundtrip(Scalar::I64(-64));
        roundtrip(Scalar::I128(i128::MIN));
        roundtrip(Scalar::U8(200));
        roundtrip(Scalar::U16(60000));
        roundtrip(Scalar::U32(u32::MAX));
        roundtrip(Scalar::U64(u64::MAX));
        roundtrip(Scalar::U128(u128::MAX));
        roundtrip(Scalar::F32(-1.5));
        roundtrip(Scalar::F64(-1.5));
        roundtrip(Scalar::Str(String::from("héllo/[world]")));
        roundtrip(Scalar::Bytes(vec![0, 1, 2, 255]));
        roundtrip(Scalar::Uuid(Uuid::from_u128(0x1234_5678)));
        roundtrip(Scalar::Date(NaiveDate::from_ymd_opt(2026, 6, 21).unwrap()));
        roundtrip(Scalar::Time(NaiveTime::from_hms_milli_opt(23, 59, 59, 250).unwrap()));
        roundtrip(Scalar::DateTime(
            NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc(),
        ));

        roundtrip(Scalar::Duration(
            TimeDelta::seconds(-90) + TimeDelta::milliseconds(-500),
        ));

        roundtrip(Scalar::Duration(TimeDelta::nanoseconds(1)));
    }

    #[test]
    fn type_str_labels_every_base_variant() {
        assert_eq!(Scalar::Null.type_str(), "null");
        assert_eq!(Scalar::Bool(true).type_str(), "bool");
        assert_eq!(Scalar::I8(0).type_str(), "i8");
        assert_eq!(Scalar::I16(0).type_str(), "i16");
        assert_eq!(Scalar::I32(0).type_str(), "i32");
        assert_eq!(Scalar::I64(0).type_str(), "i64");
        assert_eq!(Scalar::I128(0).type_str(), "i128");
        assert_eq!(Scalar::U8(0).type_str(), "u8");
        assert_eq!(Scalar::U16(0).type_str(), "u16");
        assert_eq!(Scalar::U32(0).type_str(), "u32");
        assert_eq!(Scalar::U64(0).type_str(), "u64");
        assert_eq!(Scalar::U128(0).type_str(), "u128");
        assert_eq!(Scalar::F32(0.0).type_str(), "f32");
        assert_eq!(Scalar::F64(0.0).type_str(), "f64");
        assert_eq!(Scalar::Str(String::new()).type_str(), "str");
        assert_eq!(Scalar::Bytes(vec![]).type_str(), "bytes");
        assert_eq!(Scalar::Uuid(Uuid::nil()).type_str(), "uuid");
        assert_eq!(
            Scalar::Date(NaiveDate::from_ymd_opt(2000, 1, 1).unwrap()).type_str(),
            "date"
        );
        assert_eq!(
            Scalar::Time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()).type_str(),
            "time"
        );
        assert_eq!(
            Scalar::DateTime(
                NaiveDate::from_ymd_opt(2000, 1, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc(),
            )
            .type_str(),
            "datetime"
        );

        assert_eq!(Scalar::Duration(TimeDelta::zero()).type_str(), "duration");
    }

    #[test]
    fn decode_rejects_an_unknown_tag() {
        let mut reader = Reader::new(&[0xFF]);
        let err = Scalar::decode(&mut reader).unwrap_err();

        assert!(matches!(err, AdbError::Corrupt(_)));
    }

    #[test]
    fn decode_rejects_invalid_utf8_in_a_string() {
        let mut buf = vec![tag::STR];
        codec::put_bytes(&mut buf, &[0xFF, 0xFE]);

        let err = Scalar::decode(&mut Reader::new(&buf)).unwrap_err();
        assert!(matches!(err, AdbError::Corrupt(_)));
    }

    #[cfg(feature = "bigint-as-scalar")]
    #[test]
    fn bigint_scalar_roundtrips() {
        let big = BigInt::parse_bytes(b"123456789012345678901234567890", 10).unwrap();

        roundtrip(Scalar::BigInt(BigInt::from(0)));
        roundtrip(Scalar::BigInt(BigInt::from(127)));
        roundtrip(Scalar::BigInt(BigInt::from(128)));
        roundtrip(Scalar::BigInt(BigInt::from(-129)));
        roundtrip(Scalar::BigInt(big.clone()));
        roundtrip(Scalar::BigInt(-big));
        assert_eq!(Scalar::BigInt(BigInt::from(0)).type_str(), "bigint");
    }

    #[cfg(feature = "rational-as-scalar")]
    #[test]
    fn rational_scalar_roundtrips() {
        roundtrip(Scalar::Rational(BigRational::new(BigInt::from(0), BigInt::from(1))));
        roundtrip(Scalar::Rational(BigRational::new(BigInt::from(1), BigInt::from(3))));
        roundtrip(Scalar::Rational(BigRational::new(BigInt::from(-7), BigInt::from(2))));
        assert_eq!(
            Scalar::Rational(BigRational::new(BigInt::from(0), BigInt::from(1))).type_str(),
            "rational"
        );
    }

    #[cfg(feature = "bigfloat-as-scalar")]
    #[test]
    fn bigfloat_scalar_roundtrips() {
        // Finite values and infinities compare by value, so `assert_eq` is meaningful.
        roundtrip(Scalar::BigFloat(FLOAT_ZERO));
        roundtrip(Scalar::BigFloat(BigFloat::from_f64(123.42)));
        roundtrip(Scalar::BigFloat(BigFloat::from_f64(-123.42)));
        roundtrip(Scalar::BigFloat(INF_POS));
        roundtrip(Scalar::BigFloat(INF_NEG));
        assert_eq!(Scalar::BigFloat(FLOAT_ZERO).type_str(), "bigfloat");

        // NaN never equals itself, so check the decoded flavour explicitly.
        let mut buf = Vec::new();
        Scalar::BigFloat(FLOAT_NAN).encode(&mut buf);

        let decoded = Scalar::decode(&mut Reader::new(&buf)).expect("decode");
        assert!(matches!(decoded, Scalar::BigFloat(f) if f.is_nan()));
    }
}
