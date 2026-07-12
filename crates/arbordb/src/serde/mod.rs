//! A Serde bridge for ArborDb's value codec.
//!
//! [`to_blob`] / [`from_blob`] are the primary path: a [`Serialize`](serde::Serialize)
//! value is written **straight into a value blob** (`Vec<u8>`), and a
//! [`DeserializeOwned`](serde::de::DeserializeOwned) value is read **straight back
//! out of a `&[u8]`** over the zero-copy archived cursor — with no intermediate
//! [`Value`] tree either way. This backs
//! [`store_serde_value`](crate::txn::WriteTxn::store_serde_value) and
//! [`load_serde_value`](crate::txn::ReadTxn::load_serde_value).
//!
//! [`to_value`] / [`from_value`] bridge to an owned [`Value`] (used by
//! [`Value::from_serde_value`](crate::Value::from_serde_value) and
//! [`Value::to_serde_value`](crate::Value::to_serde_value)); they are expressed in
//! terms of the blob path plus the codec's `encode` / `decode`.
//!
//! A value is stored *natively* — as objects, lists, and scalar leaves — so it is
//! indexable and navigable by `VPath`, exactly like a value built through the typed
//! `AData` path. The enum representation is externally tagged: a unit variant is its
//! name (a string leaf), and every other variant is a one-field object
//! `{ variant: payload }` (the payload being the newtype value, a list for a tuple
//! variant, or an object for a struct variant).
//!
//! Separately, [`dynamic`] implements [`Serialize`](serde::Serialize) /
//! [`Deserialize`](serde::Deserialize) for [`Value`] and [`Scalar`](crate::data::Scalar)
//! themselves, so a `Value` can be exported to any Serde format (JSON, …).

mod blob_de;
mod blob_ser;
mod dynamic;
mod functions;

pub(crate) use functions::{from_blob, from_value, to_blob, to_value};
