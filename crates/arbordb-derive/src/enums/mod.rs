//! Codegen for `#[derive(AData)]` on enums (serde-style tagging representations).
//!
//! The default is EXTERNAL tagging: the value node is an object with ONE field
//! named after the active variant's tag, holding the payload. `#[arbor(tag = ...)]`
//! selects INTERNAL tagging, `tag` + `content` ADJACENT, and `untagged` UNTAGGED
//! (see [`repr::EnumRepr`]). `store` clears the node first, so exactly one variant
//! survives a change.
//!
//! The stored tag is the variant's `rename`, else the container's `rename_all`
//! applied to the variant name, else the name verbatim. `alias` tags are accepted
//! on load only. `#[arbor(other)]` marks a unit variant as the catch-all for an
//! unknown tag; `#[arbor(expecting = "...")]` overrides the "no match" error.

mod expand;
mod load;
mod repr;
mod store;
mod variant;

pub(crate) use expand::expand_enum;
