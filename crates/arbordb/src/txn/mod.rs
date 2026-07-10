//! Opaque read and write transactions over a table.

mod query;
mod read;
mod rooted;
mod write;

pub use self::{
    query::IndexQuery,
    read::ReadTxn,
    rooted::{RootedRead, RootedWrite},
    write::WriteTxn,
};
