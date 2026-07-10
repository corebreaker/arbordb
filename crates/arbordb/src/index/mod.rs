//! Secondary indexes.
//!
//! Built incrementally: the order-preserving key encoding (the `ordered` module)
//! and the index definitions ([`IndexDef`] / [`IndexColumn`] / [`Direction`]) plus
//! the [`AIndexed`] trait are in place; the persisted registry, write-time
//! maintenance, back-fill, and the query builder follow in later sub-phases.

mod definitions;
mod id;
mod indexed;
mod key;
mod ordered;
mod pattern;

pub(crate) mod maintenance;
pub(crate) mod registry;

pub(crate) use self::id::IndexId;

pub use self::{
    definitions::{Direction, IndexColumn, IndexDef},
    indexed::AIndexed,
};
