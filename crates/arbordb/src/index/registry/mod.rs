//! The secondary-index registry, persisted in the reserved `$metadata` table.
//!
//! Every index definition lives in a single metadata value (a small blob under
//! `META_INDEX_REGISTRY_KEY`): index counts are tiny, so this keeps creation a
//! plain read-modify-write with no per-index key scheme to design. Each entry is
//! scoped by table name, so two tables may reuse an index name.

mod entry;
mod interface;
mod repository;

pub(crate) use self::{
    entry::IndexEntry,
    interface::{any, create, delete, for_table, has, lookup},
};
