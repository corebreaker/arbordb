//! The storage-engine boundary. Everything that knows the concrete key-value
//! engine (redb) lives here; nothing above this layer names a redb type.

mod entry;
mod errors;
mod functions;

pub(crate) use self::{
    entry::{dir_entry, file_entry, get_entry_kind, entry_split},
    functions::{bootstrap_metadata, child_of, data_def, fetch_entry_kind, read_entry, resolve, EntryBytes},
};

pub(crate) use self::functions::{INDEX_TABLE, META_TABLE};

pub use self::entry::{Entry, EntryKind};
