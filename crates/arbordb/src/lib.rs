//! ArborDb — a typed, transactional, indexed document store shaped like a virtual
//! filesystem.
//!
//! A table is a tree of **directories** and **files**. A *file* is one [`Value`] —
//! a tree of objects, lists and scalar leaves — serialized into a **single blob**
//! and navigated **zero-copy**: a dedicated codec reads a field straight out of
//! the engine's page bytes, without decoding the rest. A *directory* maps child
//! names to child nodes. Every vnode — file or directory — has an opaque, stable
//! [`AKey`] identity that survives renames and moves.
//!
//! A vnode is addressed by an [`APath`](path::APath), a filesystem-like path of
//! names (`users/alice`); navigation *inside* a file's value, down to a scalar
//! leaf, uses a [`VPath`](path::VPath) (names plus list indices). The underlying
//! storage engine is an implementation detail, never exposed in the public API.
//!
//! # What's here
//!
//! - A filesystem API on a [`Table`]'s transactions: `ls` / `mv` (relink, O(1), identity-preserving) / `cp` (deep copy)
//!   / `mkdir` / `rm`, plus `store` / `load` / `get` / `kind`.
//! - Concurrent [reads](txn::ReadTxn) and a single serialized [writer](txn::WriteTxn); snapshot-consistent reads,
//!   durable-on-commit writes.
//! - The dynamic [`Value`] document type, and the typed [`AData`] trait with `#[derive(AData)]` (behind the `derive`
//!   feature) and its Serde-style `#[arbor(...)]` attributes.
//! - Named, composite, optionally-unique [secondary indexes](index) with an order-preserving key encoding, declared
//!   with `#[arbor(index(...))]` or built with [`Table::create_index`], and queried with
//!   [`ReadTxn::find`](txn::ReadTxn::find).
//! - [Rooted views](txn::RootedRead) that make every path relative to a fixed root.
//! - Optional per-vnode `created` / `modified` / `accessed` timestamps (`entry-timestamps`), and user/password
//!   authentication with per-vnode ACLs and keyed-MAC tamper detection (`permissions`).
//! - An optional big-number scalar/data feature matrix (`bignum`).
//!
//! # Quick start
//!
//! ```
//! use std::collections::BTreeMap;
//! use arbordb::{data::Scalar, ArborDb, Value};
//!
//! # fn main() -> arbordb::AdbResult<()> {
//! let db = ArborDb::create_in_memory()?;
//! let users = db.open_table("users")?;
//!
//! // Writes are transactional: stage, then commit.
//! let w = users.write()?;
//! w.store_value(
//!     "alice",
//!     &Value::Node(BTreeMap::from([(
//!         String::from("age"),
//!         Value::Leaf(Scalar::I64(30)),
//!     )])),
//! )?;
//! w.commit()?;
//!
//! // Reads see committed data; a field is reached by its intra-value path.
//! let r = users.read()?;
//! assert_eq!(r.get_as::<i64>("alice", "age")?, Some(30));
//! # Ok(())
//! # }
//! ```

mod cache;
mod codec;
mod constants;
mod db;
mod engine;
mod error;
mod key;
mod table;
mod time;
mod value;
mod vnode;

#[cfg(feature = "permissions")]
mod crypto;

pub mod access;
pub mod data;
pub mod index;
pub mod path;
pub mod txn;

/// Public access-control types (`Mode`, `Rights`, `NodeAcl`) for the `permissions` feature.
#[cfg(feature = "permissions")]
pub mod acl;

#[cfg(feature = "permissions")]
pub mod perm;

/// The per-inode timestamps exposed by the `entry-timestamps` feature.
#[cfg(feature = "entry-timestamps")]
pub mod inode;

pub mod entry {
    //! Directory-listing types: the [`Entry`] rows (name + [`EntryKind`]) yielded by `ls`.

    pub use crate::engine::{Entry, EntryKind};
}

pub use self::{
    data::AData,
    db::ArborDb,
    error::{AdbError, AdbResult},
    key::AKey,
    vnode::NodeKind,
    table::Table,
    value::Value,
};

/// Derives [`AData`] for a struct, generating its `ArborXxx` / `ArborXxxMut`
/// accessors and an `ArborXxxDesc` companion. Shares the `AData` name with the
/// trait (distinct namespaces), so `use arbordb::AData;` brings both into scope.
#[cfg(feature = "derive")]
pub use arbordb_derive::AData;
