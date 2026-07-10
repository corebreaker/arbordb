//! Access traits and cursors over a single value's node tree.
//!
//! A value is one blob; navigation inside it is by [`VPath`](crate::path::VPath).
//! The [`Reader`] / [`Writer`] traits are what `AData` and the generated accessors
//! are written against — backed on reads by a zero-copy cursor over a stored blob,
//! and on writes by an in-memory value builder.

mod archived;
mod mem;
mod reader;
mod writer;

pub(crate) use self::{archived::ArchivedReader, mem::MemWriter};
pub use self::{reader::Reader, writer::Writer};
