//! Opaque read and write transactions over a table.

mod query;
mod read;
mod write;

pub use self::{query::IndexQuery, read::ReadTxn, write::WriteTxn};
