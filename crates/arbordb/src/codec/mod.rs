//! The value codec: low-level byte helpers plus ArborDb's own zero-copy value
//! format (see [`archived`]) and directory format (see [`dir`]).
//!
//! Multi-byte integers are written big-endian. Variable-length byte runs are
//! length-prefixed so the low-level encoders are self-delimiting.

mod archived;
mod dir;
mod putters;
mod read;
mod reader;

pub(crate) use self::{
    archived::{ArchivedValue, decode, encode},
    dir::{ArchivedDir, encode_dir},
    putters::{put_bytes, put_u32},
    reader::Reader,
};

// The low-level blob-emission primitives, shared by the value codec, the Serde blob
// writer, and the typed direct encoder (`AData::encode_node`).
pub(crate) use self::archived::{begin_blob, patch_root, push_leaf, push_list, push_object, push_value};

#[cfg(feature = "serde")]
pub(crate) use self::archived::ArchivedNode;
