//! The typed data model: [`Scalar`] leaves, the [`AValue`] scalar-mapping trait,
//! the composite [`AData`] trait, and the accessor types.

mod definition;
mod leaf;
mod refs;
mod scalar;
mod value;

pub use self::{
    definition::AData,
    leaf::{Leaf, LeafMut},
    refs::{AIdentifiable, AMut, ARef},
    scalar::Scalar,
    value::AValue,
};
