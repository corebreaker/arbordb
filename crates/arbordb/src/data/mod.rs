//! The typed data model: [`Scalar`] leaves, the [`AValue`] scalar-mapping trait,
//! the composite [`AData`] trait, the container adapters, and the accessor types.

mod bytes;
mod definition;
mod encode;
mod leaf;
mod map;
mod opt;
mod refs;
mod scalar;
mod seq;
mod value;

// Big-number `AData` via a `Bytes` leaf, only when a type is as-data but not as-scalar.
#[cfg(any(
    all(not(feature = "bigint-as-scalar"), feature = "bigint-as-data"),
    all(not(feature = "bigfloat-as-scalar"), feature = "bigfloat-as-data"),
    all(not(feature = "rational-as-scalar"), feature = "rational-as-data"),
))]
mod bignum;

#[cfg(any(feature = "rational-as-scalar", feature = "rational-as-data"))]
pub mod rational;

pub(crate) use self::encode::encode_data;

pub use self::{
    bytes::Bytes,
    definition::AData,
    encode::NodeEncoder,
    leaf::{Leaf, LeafMut},
    map::{Map, MapMut},
    opt::{Opt, OptMut},
    refs::{AIdentifiable, AMut, ARef},
    scalar::Scalar,
    seq::{Seq, SeqMut},
    value::AValue,
};
