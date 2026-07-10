//! ArborDb — a typed, transactional, indexed document store.
//!
//! ArborDb layers a document/tree data model over an embedded key-value engine.
//! Each stored value is one [`Value`] — a tree of objects, lists and scalar
//! leaves — serialized into a **single blob in one engine entry** and navigated
//! **zero-copy**: a dedicated codec reads a field straight out of the engine's
//! page bytes, without decoding the rest.
//!
//! A value is addressed by an [`APath`](path::APath) — a filesystem-like path of
//! names that serves directly as its storage key; navigation *inside* a value,
//! down to a scalar leaf, uses a [`VPath`](path::VPath) (names plus list indices).
//! The underlying storage engine is an implementation detail, never exposed in
//! the public API.
//!
//! This crate is under construction; the storage engine, transactions, typed
//! accessors, the `#[derive(AData)]` macro and secondary indexes land in later
//! phases.

mod value;

pub mod data;
pub mod error;
pub mod path;

pub use self::{
    error::{AdbError, AdbResult},
    value::Value,
};
