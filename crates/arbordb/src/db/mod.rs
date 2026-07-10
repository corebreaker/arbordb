//! The database handle and the database-wide shared state behind it.

mod database;
mod inner;

pub(crate) use inner::DbInner;

pub use database::ArborDb;
