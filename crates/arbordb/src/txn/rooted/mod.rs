//! Rooted transaction views.
//!
//! [`ReadTxn::rooted`](super::ReadTxn::rooted) and
//! [`WriteTxn::rooted`](super::WriteTxn::rooted) return a [`RootedRead`] /
//! [`RootedWrite`] that interprets every access path relative to a fixed root:
//! each call joins `root` with the given path and forwards to the underlying
//! transaction, so `txn.rooted("users/alice")?.get_as("", "age")` reads
//! `users/alice`'s `age`. Resolution still walks from the table root each time
//! (the root is a path prefix, not a cached key), so a view stays correct even as
//! the tree under `root` is created or replaced.
//!
//! A view borrows its transaction, so it must be dropped before the transaction
//! is committed. Views nest: `rooted` again to descend further.

mod read;
mod write;

pub use self::{read::RootedRead, write::RootedWrite};
