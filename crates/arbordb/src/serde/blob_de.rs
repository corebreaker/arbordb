//! A [`Deserializer`] that reads a value blob **directly** — over the zero-copy
//! [`ArchivedNode`] cursor, with no intermediate [`Value`](crate::value::Value) tree.
//!
//! It walks the archived blob in place: a leaf feeds the matching scalar to the
//! visitor, a list drives a `visit_seq` jumping element offsets, an object drives a
//! `visit_map` over the name-sorted entry table. The enum shape matches the rest of
//! the crate's Serde support: a unit variant is a string leaf, every other variant a
//! one-field object `{ variant: payload }`.

use crate::{
    codec::{ArchivedNode, ArchivedValue},
    data::Scalar,
    error::{AdbError, AdbResult},
    vnode::NodeKind,
};

use serde::{
    de::{self, DeserializeOwned, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor},
    Deserializer,
};

/// Reconstructs a `T` straight from a value blob, reading it zero-copy.
pub(super) fn from_blob<T: DeserializeOwned>(blob: &[u8]) -> AdbResult<T> {
    let archived = ArchivedValue::new(blob)?;

    T::deserialize(BlobDeserializer {
        node: archived.root()
    })
}

/// A [`Deserializer`] positioned at one vnode of the borrowed blob.
struct BlobDeserializer<'a> {
    /// The cursor at the vnode being read.
    node: ArchivedNode<'a>,
}

impl<'de, 'a> Deserializer<'de> for BlobDeserializer<'a> {
    type Error = AdbError;

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct
        identifier ignored_any
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        match self.node.kind()? {
            NodeKind::Leaf => scalar_into(self.node.scalar()?, visitor),
            NodeKind::List => visitor.visit_seq(SeqReader {
                node:  self.node,
                index: 0,
                len:   self.node.len()?,
            }),
            NodeKind::Object => visitor.visit_map(MapReader {
                entries: self.node.entries()?,
                index:   0,
                value:   None,
            }),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        if self.node.kind()? == NodeKind::Leaf && matches!(self.node.scalar()?, Scalar::Null) {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, AdbError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, AdbError> {
        match self.node.kind()? {
            // A unit variant is stored as its bare name.
            NodeKind::Leaf => match self.node.scalar()? {
                Scalar::Str(name) => visitor.visit_enum(EnumReader {
                    variant: name,
                    payload: None,
                }),
                _ => Err(de::Error::custom("expected an enum: a string or a one-field object")),
            },

            // Every other variant is a one-field object `{ variant: payload }`.
            NodeKind::Object => {
                let entries = self.node.entries()?;
                if entries.len() != 1 {
                    return Err(de::Error::custom("expected a one-field object for an enum variant"));
                }

                let (name, payload) = entries.into_iter().next().expect("a one-entry object has an entry");

                visitor.visit_enum(EnumReader {
                    variant: name.to_owned(),
                    payload: Some(payload),
                })
            }

            NodeKind::List => Err(de::Error::custom("expected an enum, found a list")),
        }
    }
}

/// Dispatches a leaf scalar to the matching visitor method. The non-serde-native
/// scalars (uuid, date/time, big numbers) have no Serde primitive, so they are
/// rejected rather than silently coerced.
fn scalar_into<'de, V: Visitor<'de>>(scalar: Scalar, visitor: V) -> Result<V::Value, AdbError> {
    match scalar {
        Scalar::Null => visitor.visit_unit(),
        Scalar::Bool(v) => visitor.visit_bool(v),
        Scalar::I8(v) => visitor.visit_i8(v),
        Scalar::I16(v) => visitor.visit_i16(v),
        Scalar::I32(v) => visitor.visit_i32(v),
        Scalar::I64(v) => visitor.visit_i64(v),
        Scalar::I128(v) => visitor.visit_i128(v),
        Scalar::U8(v) => visitor.visit_u8(v),
        Scalar::U16(v) => visitor.visit_u16(v),
        Scalar::U32(v) => visitor.visit_u32(v),
        Scalar::U64(v) => visitor.visit_u64(v),
        Scalar::U128(v) => visitor.visit_u128(v),
        Scalar::F32(v) => visitor.visit_f32(v),
        Scalar::F64(v) => visitor.visit_f64(v),
        Scalar::Str(v) => visitor.visit_string(v),
        Scalar::Bytes(v) => visitor.visit_byte_buf(v),
        other => Err(de::Error::custom(format!(
            "cannot deserialize a scalar of type {} via serde",
            other.type_str()
        ))),
    }
}

/// Reads a list vnode element by element, jumping each child's offset.
struct SeqReader<'a> {
    /// The list vnode.
    node:  ArchivedNode<'a>,
    /// The next element index.
    index: usize,
    /// The element count.
    len:   usize,
}

impl<'de, 'a> SeqAccess<'de> for SeqReader<'a> {
    type Error = AdbError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, AdbError> {
        if self.index >= self.len {
            return Ok(None);
        }

        let child = self
            .node
            .at(self.index)?
            .ok_or_else(|| AdbError::Corrupt(String::from("a list element is missing")))?;
        self.index += 1;

        seed.deserialize(BlobDeserializer {
            node: child
        })
        .map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.len - self.index)
    }
}

/// Reads an object vnode as a map, over its name-sorted entry table.
struct MapReader<'a> {
    /// The `(field name, child vnode)` entries, in name order.
    entries: Vec<(&'a str, ArchivedNode<'a>)>,
    /// The next entry index.
    index:   usize,
    /// The child awaiting its `next_value_seed` (set by `next_key_seed`).
    value:   Option<ArchivedNode<'a>>,
}

impl<'de, 'a> MapAccess<'de> for MapReader<'a> {
    type Error = AdbError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, AdbError> {
        if self.index >= self.entries.len() {
            return Ok(None);
        }

        let (name, child) = self.entries[self.index];
        self.value = Some(child);
        self.index += 1;

        seed.deserialize(MapKeyDeserializer {
            key: name
        })
        .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, AdbError> {
        let child = self.value.take().expect("next_value_seed follows next_key_seed");

        seed.deserialize(BlobDeserializer {
            node: child
        })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len() - self.index)
    }
}

/// Deserializes a map key from its stored field name: a string as-is, or a scalar
/// parsed from it (so a `HashMap<u32, _>` or a unit-enum key round-trips).
struct MapKeyDeserializer<'a> {
    /// The field name to interpret as the key.
    key: &'a str,
}

/// Parses `key` as `T`, mapping a failure to a descriptive error.
fn parse_key<T: std::str::FromStr>(key: &str, ty: &str) -> Result<T, AdbError> {
    key.parse()
        .map_err(|_| de::Error::custom(format!("map key '{key}' is not a valid {ty}")))
}

impl<'de, 'a> Deserializer<'de> for MapKeyDeserializer<'a> {
    type Error = AdbError;

    serde::forward_to_deserialize_any! {
        char str string bytes byte_buf unit unit_struct seq tuple tuple_struct
        map struct identifier ignored_any
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_str(self.key)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_bool(parse_key(self.key, "bool")?)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_i8(parse_key(self.key, "i8")?)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_i16(parse_key(self.key, "i16")?)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_i32(parse_key(self.key, "i32")?)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_i64(parse_key(self.key, "i64")?)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_i128(parse_key(self.key, "i128")?)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_u8(parse_key(self.key, "u8")?)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_u16(parse_key(self.key, "u16")?)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_u32(parse_key(self.key, "u32")?)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_u64(parse_key(self.key, "u64")?)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_u128(parse_key(self.key, "u128")?)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_f32(parse_key(self.key, "f32")?)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_f64(parse_key(self.key, "f64")?)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, AdbError> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, AdbError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, AdbError> {
        visitor.visit_enum(EnumReader {
            variant: self.key.to_owned(),
            payload: None,
        })
    }
}

/// Reads an externally-tagged enum: the variant name plus its optional payload node.
struct EnumReader<'a> {
    /// The variant name.
    variant: String,
    /// The payload node, or `None` for a unit variant.
    payload: Option<ArchivedNode<'a>>,
}

impl<'de, 'a> EnumAccess<'de> for EnumReader<'a> {
    type Error = AdbError;
    type Variant = VariantReader<'a>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), AdbError> {
        let de = serde::de::value::StrDeserializer::<AdbError>::new(&self.variant);
        let variant = seed.deserialize(de)?;

        Ok((
            variant,
            VariantReader {
                payload: self.payload
            },
        ))
    }
}

/// Reads the payload of one enum variant.
struct VariantReader<'a> {
    /// The payload node, or `None` for a unit variant.
    payload: Option<ArchivedNode<'a>>,
}

impl<'de, 'a> VariantAccess<'de> for VariantReader<'a> {
    type Error = AdbError;

    fn unit_variant(self) -> Result<(), AdbError> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, AdbError> {
        match self.payload {
            Some(node) => seed.deserialize(BlobDeserializer {
                node,
            }),
            None => Err(de::Error::custom("a newtype variant needs a payload")),
        }
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, AdbError> {
        match self.payload {
            Some(node) => BlobDeserializer {
                node,
            }
            .deserialize_any(visitor),
            None => Err(de::Error::custom("a tuple variant needs a payload")),
        }
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, AdbError> {
        match self.payload {
            Some(node) => BlobDeserializer {
                node,
            }
            .deserialize_any(visitor),
            None => Err(de::Error::custom("a struct variant needs a payload")),
        }
    }
}
