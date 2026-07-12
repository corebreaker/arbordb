//! The [`AValue`] trait, mapping Rust types to and from a [`Scalar`].

use super::{
    definition::AData,
    leaf::{Leaf, LeafMut},
    Scalar,
};

use crate::{
    access::{Reader, Writer},
    error::{AdbError, AdbResult},
    path::VPath,
    AKey,
};

use chrono::{DateTime, NaiveDate, NaiveTime, TimeDelta, Utc};
use uuid::Uuid;

/// A Rust type that maps to and from a single persisted [`Scalar`] (leaf) value.
pub trait AValue: Sized {
    /// Converts this value into its scalar representation.
    fn to_scalar(&self) -> Scalar;

    /// Reconstructs this value from a stored scalar.
    fn from_scalar(scalar: &Scalar) -> AdbResult<Self>;
}

impl AValue for Scalar {
    fn to_scalar(&self) -> Scalar {
        self.clone()
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        Ok(scalar.clone())
    }
}

macro_rules! scalar_value {
    ($t:ty, $variant:ident, $name:literal) => {
        impl AValue for $t {
            fn to_scalar(&self) -> Scalar {
                Scalar::$variant(self.clone())
            }

            fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
                match scalar {
                    Scalar::$variant(v) => Ok(v.clone()),
                    other => Err(AdbError::TypeMismatch {
                        expected: $name,
                        found:    other.type_str(),
                    }),
                }
            }
        }
    };
}

scalar_value!(bool, Bool, "bool");
scalar_value!(i8, I8, "i8");
scalar_value!(i16, I16, "i16");
scalar_value!(i32, I32, "i32");
scalar_value!(i64, I64, "i64");
scalar_value!(i128, I128, "i128");
scalar_value!(u8, U8, "u8");
scalar_value!(u16, U16, "u16");
scalar_value!(u32, U32, "u32");
scalar_value!(u64, U64, "u64");
scalar_value!(u128, U128, "u128");
scalar_value!(f32, F32, "f32");
scalar_value!(f64, F64, "f64");
scalar_value!(String, Str, "str");
scalar_value!(Vec<u8>, Bytes, "bytes");
scalar_value!(Uuid, Uuid, "uuid");
scalar_value!(NaiveDate, Date, "date");
scalar_value!(NaiveTime, Time, "time");
scalar_value!(DateTime<Utc>, DateTime, "datetime");
scalar_value!(TimeDelta, Duration, "duration");

#[cfg(feature = "bigint-as-scalar")]
scalar_value!(num_bigint::BigInt, BigInt, "bigint");

#[cfg(feature = "bigfloat-as-scalar")]
scalar_value!(num_bigfloat::BigFloat, BigFloat, "bigfloat");

#[cfg(feature = "rational-as-scalar")]
scalar_value!(num_rational::BigRational, Rational, "rational");

// Platform-dependent integer widths are normalised to a fixed width so the
// on-disk format is portable.
impl AValue for usize {
    fn to_scalar(&self) -> Scalar {
        Scalar::U64(*self as u64)
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        match scalar {
            Scalar::U64(v) => Ok(*v as usize),
            other => Err(AdbError::TypeMismatch {
                expected: "usize",
                found:    other.type_str(),
            }),
        }
    }
}

impl AValue for isize {
    fn to_scalar(&self) -> Scalar {
        Scalar::I64(*self as i64)
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        match scalar {
            Scalar::I64(v) => Ok(*v as isize),
            other => Err(AdbError::TypeMismatch {
                expected: "isize",
                found:    other.type_str(),
            }),
        }
    }
}

impl AValue for AKey {
    fn to_scalar(&self) -> Scalar {
        Scalar::Uuid((*self).into())
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        match scalar {
            Scalar::Uuid(uuid) => Ok(AKey::from_bytes(*uuid.as_bytes())),
            Scalar::Null => Ok(AKey::ROOT),
            Scalar::Bytes(v) => AKey::try_from_bytes(v),
            Scalar::U128(v) => Ok(AKey::from(*v)),
            other => Err(AdbError::TypeMismatch {
                expected: "akey",
                found:    other.type_str(),
            }),
        }
    }
}

impl<T: AValue> AValue for Option<T> {
    fn to_scalar(&self) -> Scalar {
        match self {
            Some(v) => v.to_scalar(),
            None => Scalar::Null,
        }
    }

    fn from_scalar(scalar: &Scalar) -> AdbResult<Self> {
        match scalar {
            Scalar::Null => Ok(None),
            other => Ok(Some(T::from_scalar(other)?)),
        }
    }
}

// Every scalar type is also `AData`: it stores as a single leaf, so a scalar field
// and a composite field decompose through the same trait. The impls are concrete
// (not a blanket over `AValue`) to stay coherent with the container and derived impls.
macro_rules! scalar_adata {
    ($t:ty) => {
        impl AData for $t {
            type Mut<'t> = LeafMut<'t, $t>;
            type Ref<'t> = Leaf<'t, $t>;

            fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
                writer.put_scalar(at, self.to_scalar())
            }

            fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
                match reader.scalar_at(at)? {
                    Some(scalar) => <$t>::from_scalar(&scalar),
                    None => Err(AdbError::PathNotFound(at.clone())),
                }
            }
        }
    };
}

scalar_adata!(bool);
scalar_adata!(i8);
scalar_adata!(i16);
scalar_adata!(i32);
scalar_adata!(i64);
scalar_adata!(i128);
scalar_adata!(u8);
scalar_adata!(u16);
scalar_adata!(u32);
scalar_adata!(u64);
scalar_adata!(u128);
scalar_adata!(f32);
scalar_adata!(f64);
scalar_adata!(usize);
scalar_adata!(isize);
scalar_adata!(String);
scalar_adata!(Uuid);
scalar_adata!(NaiveDate);
scalar_adata!(NaiveTime);
scalar_adata!(DateTime<Utc>);
scalar_adata!(TimeDelta);

// When a big-number type is BOTH a native `Scalar` and an as-data type, it stores
// as its native leaf. The as-data-only path (`Bytes` leaf) lives in `bignum`.
#[cfg(all(feature = "bigint-as-scalar", feature = "bigint-as-data"))]
scalar_adata!(num_bigint::BigInt);

#[cfg(all(feature = "bigfloat-as-scalar", feature = "bigfloat-as-data"))]
scalar_adata!(num_bigfloat::BigFloat);

#[cfg(all(feature = "rational-as-scalar", feature = "rational-as-data"))]
scalar_adata!(num_rational::BigRational);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_is_its_own_avalue() {
        let scalar = Scalar::I32(5);

        assert_eq!(scalar.to_scalar(), scalar);
        assert_eq!(Scalar::from_scalar(&scalar).unwrap(), scalar);
    }

    #[test]
    fn platform_widths_normalise_to_fixed_widths() {
        assert_eq!(7usize.to_scalar(), Scalar::U64(7));
        assert_eq!(usize::from_scalar(&Scalar::U64(7)).unwrap(), 7);
        assert!(matches!(
            usize::from_scalar(&Scalar::I32(1)),
            Err(AdbError::TypeMismatch { .. })
        ));

        assert_eq!((-7isize).to_scalar(), Scalar::I64(-7));
        assert_eq!(isize::from_scalar(&Scalar::I64(-7)).unwrap(), -7);
        assert!(matches!(
            isize::from_scalar(&Scalar::U8(1)),
            Err(AdbError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn akey_reads_from_several_scalar_flavours() {
        let key = AKey::from(0x1234u128);

        assert_eq!(key.to_scalar(), Scalar::Uuid(key.into()));
        assert_eq!(AKey::from_scalar(&Scalar::Uuid(key.into())).unwrap(), key);
        assert_eq!(AKey::from_scalar(&Scalar::Null).unwrap(), AKey::ROOT);
        assert_eq!(
            AKey::from_scalar(&Scalar::Bytes(key.into_bytes().to_vec())).unwrap(),
            key
        );
        assert_eq!(AKey::from_scalar(&Scalar::U128(0x1234)).unwrap(), key);
        assert!(matches!(
            AKey::from_scalar(&Scalar::Bool(true)),
            Err(AdbError::TypeMismatch { .. })
        ));
    }

    #[test]
    fn option_maps_none_to_null_and_back() {
        assert_eq!(Some(3i32).to_scalar(), Scalar::I32(3));
        assert_eq!(Option::<i32>::None.to_scalar(), Scalar::Null);

        assert_eq!(Option::<i32>::from_scalar(&Scalar::Null).unwrap(), None);
        assert_eq!(Option::<i32>::from_scalar(&Scalar::I32(3)).unwrap(), Some(3));
    }

    // When both features are on (e.g. `--all-features`), a big number stores as its
    // native `Scalar` leaf through `AData` — the as-data `Bytes` path is compiled out.
    #[cfg(all(feature = "bigint-as-scalar", feature = "bigint-as-data"))]
    #[test]
    fn bigint_is_adata_via_a_native_scalar_leaf() {
        use crate::ArborDb;
        use num_bigint::BigInt;

        let big = BigInt::parse_bytes(b"123456789012345678901234567890", 10).unwrap();

        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        {
            let w = table.write().unwrap();
            w.store::<BigInt>("x", &big).unwrap();
            w.commit().unwrap();
        }

        // Round-trips through `from_scalar(Scalar::BigInt)`, so it was stored as the
        // native scalar, not as a `Bytes` leaf.
        let r = table.read().unwrap();
        assert_eq!(r.load::<BigInt>("x").unwrap(), Some(big));
    }
}
