//! The [`AData`] composite trait.

use super::refs::{AMut, ARef};
use crate::{
    access::{Reader, Writer},
    error::AdbResult,
    path::VPath,
};

/// A type that stores into, and loads from, one value's vnode tree.
///
/// Implemented automatically by `#[derive(AData)]`. Scalars implement it too (as a
/// single leaf), so decomposition is uniform: every field is stored and loaded
/// through this same trait, at a [`VPath`] relative to the value's root.
pub trait AData: Sized {
    /// Write accessor produced by `WriteTxn::fetch_mut` (an `ArborXxxMut` or
    /// [`LeafMut`](super::LeafMut)).
    type Mut<'t>: AMut<'t>;

    /// Read accessor produced by `ReadTxn::fetch` (an `ArborXxx` or [`Leaf`](super::Leaf)).
    type Ref<'t>: ARef<'t>;

    /// Writes `self` into the subtree rooted at `at`.
    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()>;

    /// Reconstructs a value from the subtree rooted at `at`.
    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self>;
}
