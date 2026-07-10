//! The storage-engine boundary. Everything that knows the concrete key-value
//! engine (redb) lives here; nothing above this layer names a redb type.

mod entry;
mod errors;
mod functions;

pub use self::entry::EntryKind;
pub(crate) use self::{
    entry::{dir_entry, file_entry, split},
    functions::{bootstrap_metadata, child_of, data_def, entry_kind, read_entry, resolve, EntryBytes},
};

pub(crate) use self::functions::{INDEX_TABLE, META_TABLE};
