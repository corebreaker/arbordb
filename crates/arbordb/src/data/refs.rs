//! The accessor marker traits: identity and construction.
//!
//! An accessor pairs a shared cursor with a base [`VPath`] inside one value. These
//! traits are the contract the generated `ArborXxx` / `ArborXxxMut` types (and the
//! built-in [`Leaf`](super::Leaf) / [`LeafMut`](super::LeafMut)) implement.

use crate::{
    access::{Reader, Writer},
    path::VPath,
};

use std::sync::Arc;

/// An accessor that knows its own location inside the value.
pub trait AIdentifiable {
    /// The accessor's base path within the value.
    fn path(&self) -> &VPath;
}

/// A read accessor, opened over a shared [`Reader`] at a base path.
pub trait ARef<'t>: AIdentifiable + Sized {
    /// Opens the accessor rooted at `base`.
    fn open(reader: Arc<dyn Reader + 't>, base: VPath) -> Self;
}

/// A write accessor, opened over a shared [`Writer`] at a base path.
pub trait AMut<'t>: AIdentifiable + Sized {
    /// Opens the accessor rooted at `base`.
    fn open(writer: Arc<dyn Writer + 't>, base: VPath) -> Self;
}
