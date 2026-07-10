//! Read-only rendering of a [`Value`](crate::Value) into textual formats.
//!
//! The [`JsonExporter`] / [`YamlExporter`] traits (in `exporter`) are the public
//! surface; a [`ReadTxn`](crate::txn::ReadTxn) renders the value stored at an
//! access path, and a [`Value`](crate::Value) renders the in-memory subtree at an
//! intra-value path. Leaf scalars are projected to text in `scalar` (the only
//! lossy step); everything but the traits is internal.
//!
//! The rendering is hand-written and dependency-free, mirroring ArborDb's own
//! zero-copy codec: it borrows no external JSON/YAML crate. It is one-directional
//! — ArborDb never parses an export back in.
//!
//! ```
//! use arbordb::{data::Scalar, export::JsonExporter, ArborDb, Value};
//!
//! # fn main() -> arbordb::AdbResult<()> {
//! let db = ArborDb::create_in_memory()?;
//! let table = db.open_table("data")?;
//!
//! let w = table.write()?;
//! w.store_value("greeting", &Value::Leaf(Scalar::Str(String::from("hello"))))?;
//! w.commit()?;
//!
//! let r = table.read()?;
//! assert_eq!(r.export_to_json("greeting", None)?, "\"hello\"");
//! # Ok(())
//! # }
//! ```

mod base64;
mod exporter;
mod json;
mod scalar;
mod string;
mod yaml;

pub use self::exporter::{JsonExporter, YamlExporter};
