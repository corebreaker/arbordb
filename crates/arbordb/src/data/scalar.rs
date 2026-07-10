//! Scalar leaf values: the [`Scalar`] enum.
//!
//! Its round-trippable byte encoding lands with the value codec; this module is
//! the type itself, the one dynamic representation of any value ArborDb can store
//! in a leaf. Rust types map to and from it through the [`AValue`](super::AValue) trait.

use uuid::Uuid;
use chrono::{DateTime, NaiveDate, NaiveTime, TimeDelta, Utc};

#[cfg(any(feature = "bigint-as-scalar", feature = "rational-as-scalar"))]
use num_bigint::BigInt;

#[cfg(feature = "bigfloat-as-scalar")]
use num_bigfloat::BigFloat;

#[cfg(feature = "rational-as-scalar")]
use num_rational::BigRational;

/// A persisted scalar value: the content of a leaf node.
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

    /// A free-standing UUID value (not a node key).
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(feature = "bigint-as-scalar")]
    #[test]
    fn type_str_labels_bigint() {
        assert_eq!(Scalar::BigInt(BigInt::from(0)).type_str(), "bigint");
    }

    #[cfg(feature = "bigfloat-as-scalar")]
    #[test]
    fn type_str_labels_bigfloat() {
        assert_eq!(Scalar::BigFloat(num_bigfloat::ZERO).type_str(), "bigfloat");
    }

    #[cfg(feature = "rational-as-scalar")]
    #[test]
    fn type_str_labels_rational() {
        assert_eq!(
            Scalar::Rational(BigRational::new(BigInt::from(0), BigInt::from(1))).type_str(),
            "rational"
        );
    }
}
