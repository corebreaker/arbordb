//! The enum representation (serde-style tagging) and its per-repr codegen helpers.
//!
//! `External` (the default) keys the payload under the variant tag; `Internal`
//! stores the tag in a named field alongside a flattened payload; `Adjacent`
//! stores the tag and the payload in two named fields; `Untagged` stores the
//! payload bare and, on load, tries each variant in declaration order.

use crate::attr::ContainerAttrs;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Ident, Result as SynResult};

/// How an enum maps onto the tree; `External` is the default.
pub(super) enum EnumRepr {
    /// `{ Variant: payload }` — the object's single key is the tag.
    External,
    /// `{ <tag>: "Variant", <content>: payload }` — tag and payload in named fields.
    Adjacent {
        /// The field name holding the variant tag.
        tag:     String,
        /// The field name holding the payload.
        content: String,
    },
    /// `{ <tag>: "Variant", ..payload }` — tag and flattened payload in one object;
    /// tuple/newtype elements are keyed by their decimal index (`"0"`, `"1"`, …).
    Internal {
        /// The field name holding the variant tag.
        tag: String,
    },
    /// Bare payload, no tag; on load each variant is tried in declaration order.
    Untagged,
}

impl EnumRepr {
    /// Derives the representation from the container attributes, rejecting the
    /// combinations that cannot hold.
    pub(super) fn from_container(container: &ContainerAttrs, name: &Ident) -> SynResult<Self> {
        match (container.tag(), container.content(), container.untagged()) {
            (None, None, false) => Ok(Self::External),
            (Some(tag), Some(content), false) => Ok(Self::Adjacent {
                tag:     tag.to_string(),
                content: content.to_string(),
            }),
            (Some(tag), None, false) => Ok(Self::Internal {
                tag: tag.to_string()
            }),
            // no-coverage:start — attribute-validation error (invalid input never compiles)
            (None, Some(_), false) => Err(Error::new(name.span(), "`content` requires `tag`")),
            // no-coverage:stop
            (None, None, true) => Ok(Self::Untagged),
            // no-coverage:start — attribute-validation error (invalid input never compiles)
            (_, _, true) => Err(Error::new(
                name.span(),
                "`untagged` cannot be combined with `tag`/`content`",
            )),
            // no-coverage:stop
        }
    }

    /// Whether the enum stores no tag (payload bare, variants tried in order).
    pub(super) fn is_untagged(&self) -> bool {
        matches!(self, Self::Untagged)
    }

    /// Whether the tag lives inside the payload object (internal tagging).
    pub(super) fn is_internal(&self) -> bool {
        matches!(self, Self::Internal { .. })
    }

    /// The top-of-`store` statements after `remove`: object-shape the vnode for the
    /// tag-based reprs; leave it to each arm for `Untagged`.
    pub(super) fn store_prelude(&self) -> TokenStream {
        match self {
            Self::Untagged => quote! {},
            _ => quote! {
                ::arbordb::access::Writer::ensure_container(writer, at, false)?;
            },
        }
    }

    /// Store statement writing the variant tag — empty for External/Untagged,
    /// where there is no separate tag field.
    pub(super) fn tag_store(&self, variant_tag: &str) -> TokenStream {
        match self {
            Self::External | Self::Untagged => quote! {},
            Self::Adjacent {
                tag, ..
            }
            | Self::Internal {
                tag,
            } => quote! {
                ::arbordb::data::AData::store(
                    &::std::string::String::from(#variant_tag),
                    writer,
                    &at.child_name(#tag),
                )?;
            },
        }
    }

    /// Store statement(s) for a unit variant's body.
    pub(super) fn unit_store(&self, variant_tag: &str) -> TokenStream {
        match self {
            // External: the tag IS the object key, carrying a `Null` value.
            Self::External => quote! {
                ::arbordb::access::Writer::put_scalar(
                    writer,
                    &at.child_name(#variant_tag),
                    ::arbordb::data::Scalar::Null,
                )?;
            },
            // Untagged: a bare `Null` at the vnode itself.
            Self::Untagged => quote! {
                ::arbordb::access::Writer::put_scalar(writer, at, ::arbordb::data::Scalar::Null)?;
            },
            // Tagged: just the tag field (a unit variant has no payload).
            Self::Adjacent {
                ..
            }
            | Self::Internal {
                ..
            } => self.tag_store(variant_tag),
        }
    }

    /// The base path a non-unit variant's payload is written under. Internal
    /// flattens the payload and is handled separately.
    pub(super) fn payload_base_store(&self, variant_tag: &str) -> TokenStream {
        match self {
            Self::External => quote! { at.child_name(#variant_tag) },
            Self::Adjacent {
                content, ..
            } => quote! { at.child_name(#content) },
            Self::Untagged => quote! { at.clone() },
            Self::Internal {
                ..
            } => unreachable!("internal tagging is handled by internal_store_arm"),
        }
    }

    /// Statements binding `tag: String` ahead of the load match (tag-based reprs only).
    pub(super) fn tag_load(&self) -> TokenStream {
        match self {
            Self::External => quote! {
                let tag = ::arbordb::access::Reader::keys_at(reader, at)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        ::arbordb::AdbError::Corrupt(::std::string::String::from("enum vnode has no variant tag"))
                    })?;
            },
            Self::Adjacent {
                tag, ..
            }
            | Self::Internal {
                tag,
            } => quote! {
                let tag = <::std::string::String as ::arbordb::data::AData>::load(reader, &at.child_name(#tag))?;
            },
            Self::Untagged => unreachable!("untagged enums do not match on a tag"),
        }
    }

    /// The base path a non-unit variant's payload is read from (external/adjacent only).
    pub(super) fn payload_base_load(&self) -> TokenStream {
        match self {
            Self::External => quote! { at.child_name(tag.as_str()) },
            Self::Adjacent {
                content, ..
            } => quote! { at.child_name(#content) },
            Self::Internal {
                ..
            }
            | Self::Untagged => unreachable!("handled by internal_load_arm / untagged_arm"),
        }
    }

    /// The body of the accessor `variant()`; `handle` is `self.reader`/`self.writer`.
    pub(super) fn variant_body(&self, handle: TokenStream) -> TokenStream {
        match self {
            Self::External => quote! {
                ::arbordb::access::Reader::keys_at(&#handle, &self.base)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        ::arbordb::AdbError::Corrupt(::std::string::String::from("enum vnode has no variant tag"))
                    })
            },
            Self::Adjacent {
                tag, ..
            }
            | Self::Internal {
                tag,
            } => quote! {
                <::std::string::String as ::arbordb::data::AData>::load(&#handle, &self.base.child_name(#tag))
            },
            Self::Untagged => quote! {
                ::core::result::Result::Err(::arbordb::AdbError::CannotAccess(
                    ::std::string::String::from("an untagged enum stores no variant tag"),
                ))
            },
        }
    }
}
