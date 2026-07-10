//! The typed data model: [`Scalar`] leaves and the [`AValue`] scalar-mapping trait.
//!
//! The composite `AData` trait, the container adapters, and the accessor types
//! land with the typed layer in a later phase.

mod scalar;
mod value;

pub use self::{scalar::Scalar, value::AValue};
