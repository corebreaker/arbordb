//! Paths: addresses into the stored data.
//!
//! Two path types split along the storage boundary:
//!
//! - An [`APath`] addresses a whole stored [`Value`](crate::Value) — a filesystem-like slash-separated address of
//!   **names only** (`users/alice`), with no list index. It is encoded straight into the storage key.
//! - A [`VPath`] navigates **inside** a value, to a vnode or a scalar leaf. Object fields are named (`a/b`); list
//!   elements are indexed (`a/t[5]`), the index binding to the preceding name without a separator.
//!
//! Both normalize `.` (current, dropped) and `..` (parent, removes the preceding
//! segment) at parse time and never store them; a `..` above the root is an error.
//! The `/` operator appends to a `VPath`: another path joins segment-wise
//! (`a / b`), a string adds one field name (`a / "x"`). See [`VPath::join`] /
//! [`PathTail`].
//!
//! Path-addressed methods accept either kind by trait, never a bare string only:
//! filesystem methods take [`IntoArborPath`] (an [`APath`] or a string), value-side
//! methods take [`IntoValuePath`] (a [`VPath`] or a string). So a pre-built path —
//! including one assembled with `/` — and a string literal are interchangeable.

mod a_path;
mod functions;
mod into_arbor_path;
mod into_value_path;
mod segment;
mod tail;
mod v_path;

pub use self::{
    a_path::APath,
    into_arbor_path::IntoArborPath,
    into_value_path::IntoValuePath,
    segment::Segment,
    tail::PathTail,
    v_path::VPath,
};
