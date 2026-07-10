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
