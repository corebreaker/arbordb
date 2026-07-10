//! The typed data model: [`Scalar`] leaves, the [`AValue`] scalar-mapping trait,
//! the composite [`AData`] trait, the container adapters, and the accessor types.

mod bytes;
mod definition;
mod leaf;
mod map;
mod opt;
mod refs;
mod scalar;
mod seq;
mod value;

pub use self::{
    bytes::Bytes,
    definition::AData,
    leaf::{Leaf, LeafMut},
    map::{Map, MapMut},
    opt::{Opt, OptMut},
    refs::{AIdentifiable, AMut, ARef},
    scalar::Scalar,
    seq::{Seq, SeqMut},
    value::AValue,
};
