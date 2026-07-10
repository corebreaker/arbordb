//! ArborDb — a typed, transactional, indexed document store shaped like a virtual
//! filesystem.
//!
//! A table is a tree of **directories** and **files**. A *file* is one [`Value`] —
//! a tree of objects, lists and scalar leaves — serialized into a **single blob**
//! and navigated **zero-copy**: a dedicated codec reads a field straight out of
//! the engine's page bytes, without decoding the rest. A *directory* maps child
//! names to child nodes. Every node — file or directory — has an opaque, stable
//! [`AKey`] identity that survives renames and moves.
//!
//! A node is addressed by an [`APath`](path::APath), a filesystem-like path of
//! names (`users/alice`); navigation *inside* a file's value, down to a scalar
//! leaf, uses a [`VPath`](path::VPath) (names plus list indices). The underlying
//! storage engine is an implementation detail, never exposed in the public API.
//!
//! This crate is under construction; the storage engine, the filesystem API
//! (`ls`/`mv`/`cp`/…), transactions, typed accessors, the `#[derive(AData)]`
//! macro and secondary indexes land in later phases.

mod cache;
mod codec;
mod constants;
mod datetime;
mod db;
mod engine;
mod key;
mod node;
mod table;
mod value;

pub mod access;
pub mod data;
pub mod error;
// The index layer is built bottom-up; several entry points (and the registry
// facade re-exports) are wired into the write and query paths in later
// sub-phases, so silence dead-code / unused-imports until then.
#[allow(dead_code, unused_imports)]
pub mod index;
pub mod path;
pub mod txn;

pub use self::{
    data::AData,
    db::ArborDb,
    engine::EntryKind,
    error::{AdbError, AdbResult},
    key::AKey,
    node::NodeKind,
    table::Table,
    value::Value,
};

/// Derives [`AData`] for a struct, generating its `ArborXxx` / `ArborXxxMut`
/// accessors and an `ArborXxxDesc` companion. Shares the `AData` name with the
/// trait (distinct namespaces), so `use arbordb::AData;` brings both into scope.
#[cfg(feature = "derive")]
pub use arbordb_derive::AData;
