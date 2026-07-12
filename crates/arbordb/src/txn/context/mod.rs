//! The virtual-filesystem write helpers backing the public [`WriteTxn`] ops.
//!
//! These routines operate directly on the raw [`crate::txn::context::DataTable`] — resolving,
//! linking, rewriting, and cascading over directory and file nodes — rather
//! than on `&self`. Grouping them here keeps them off [`WriteTxn`]'s surface
//! while letting each public op compose them.

mod ctx;

pub(super) mod table;

pub(super) use ctx::Context;
