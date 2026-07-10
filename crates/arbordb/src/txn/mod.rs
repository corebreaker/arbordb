//! Opaque read and write transactions over a table.

mod read;
mod write;

pub use self::{read::ReadTxn, write::WriteTxn};
