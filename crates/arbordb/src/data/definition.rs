//! The [`AData`] composite trait.

use super::{
    encode::NodeEncoder,
    refs::{AMut, ARef},
};

use crate::{
    access::{MemWriter, Reader, Writer},
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

    /// Emits `self` as one vnode into `enc`, children first, returning its offset —
    /// the direct encode path behind `WriteTxn::store`, which bypasses the
    /// intermediate [`Value`](crate::Value) tree.
    ///
    /// The default builds a `Value` through [`store`](Self::store) and encodes that,
    /// so every implementor is correct with no extra work; the scalars, containers,
    /// and simple derived structs override it to emit straight into the blob (a plain
    /// `#[derive(AData)]` struct with a `flatten` or `store_with` field keeps the
    /// default). An override MUST produce the byte-for-byte blob the default would.
    #[doc(hidden)]
    fn encode_node(&self, enc: &mut NodeEncoder) -> AdbResult<u32> {
        let writer = MemWriter::new();
        self.store(&writer, &VPath::root())?;

        Ok(enc.value(&writer.into_value()))
    }
}
