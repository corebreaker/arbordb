//! Parsing and codegen for the `#[arbor(index(...))]` container attribute.

mod column_spec;
mod index_attr;
mod indexed_impl;
mod item;

pub(crate) use self::{index_attr::IndexAttr, indexed_impl::indexed_impl};
