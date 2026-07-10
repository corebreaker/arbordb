//! Parsing of the `#[arbor(...)]` attribute namespace.
//!
//! One struct per attribute site — [`ContainerAttrs`] on the type, [`FieldAttrs`]
//! on a struct field, [`VariantAttrs`] on an enum variant — plus the shared
//! `RenameRule` casing set. The codegen modules consume the parsed attributes;
//! they never re-parse the `syn` AST for naming.

mod container;
mod field;
mod rename;
mod variant;

pub(crate) use container::ContainerAttrs;
pub(crate) use field::FieldAttrs;
pub(crate) use variant::VariantAttrs;
