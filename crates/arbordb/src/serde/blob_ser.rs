//! A [`Serializer`] that writes ArborDb's value-blob codec **directly** — no
//! intermediate [`Value`](crate::value::Value) tree.
//!
//! It mirrors the codec's `encode`: the blob is laid out children-first with
//! absolute offsets, so each `serialize_*` writes its subtree into the shared buffer
//! and returns that subtree's offset (`Ok = u32`). A container gathers its children's
//! offsets, then writes its own offset table via the shared codec primitives — the
//! same ones `encode` uses, so the output is a fully valid value blob.
//!
//! Enums follow the same externally-tagged shape as the rest of the crate's Serde
//! support: a unit variant is a string leaf, and every other variant is a one-field
//! object `{ variant: payload }`.

use crate::{codec, data::Scalar, error::AdbError, error::AdbResult};
use serde::{
    ser::{
        Error as _,
        Impossible,
        Serialize,
        SerializeMap,
        SerializeSeq,
        SerializeStruct,
        SerializeStructVariant,
        SerializeTuple,
        SerializeTupleStruct,
        SerializeTupleVariant,
    },
    Serializer,
};

use std::collections::BTreeMap;

/// Serializes `value` straight into a value blob (header included).
pub(super) fn to_blob<T: Serialize + ?Sized>(value: &T) -> AdbResult<Vec<u8>> {
    let mut buf = codec::begin_blob();
    let root = value.serialize(BlobSerializer {
        buf: &mut buf
    })?;
    codec::patch_root(&mut buf, root);

    Ok(buf)
}

/// Writes `entries` (a name-sorted, deduplicated map) as an object vnode.
fn finish_object(buf: &mut Vec<u8>, entries: &BTreeMap<String, u32>) -> u32 {
    let pairs: Vec<(&str, u32)> = entries.iter().map(|(name, off)| (name.as_str(), *off)).collect();

    codec::push_object(buf, &pairs)
}

/// Wraps an already-written `payload` node as the one-field object `{ variant: payload }`.
fn wrap_variant(buf: &mut Vec<u8>, variant: &str, payload: u32) -> u32 {
    codec::push_object(buf, &[(variant, payload)])
}

/// A [`Serializer`] whose `Ok` is the absolute offset of the node it wrote into `buf`.
pub(super) struct BlobSerializer<'b> {
    /// The blob being built; every node is appended here.
    buf: &'b mut Vec<u8>,
}

impl<'b> Serializer for BlobSerializer<'b> {
    type Error = AdbError;
    type Ok = u32;
    type SerializeMap = MapBuilder<'b>;
    type SerializeSeq = SeqBuilder<'b>;
    type SerializeStruct = StructBuilder<'b>;
    type SerializeStructVariant = StructVariantBuilder<'b>;
    type SerializeTuple = SeqBuilder<'b>;
    type SerializeTupleStruct = SeqBuilder<'b>;
    type SerializeTupleVariant = TupleVariantBuilder<'b>;

    fn serialize_bool(self, v: bool) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Bool(v)))
    }

    fn serialize_i8(self, v: i8) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::I8(v)))
    }

    fn serialize_i16(self, v: i16) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::I16(v)))
    }

    fn serialize_i32(self, v: i32) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::I32(v)))
    }

    fn serialize_i64(self, v: i64) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::I64(v)))
    }

    fn serialize_i128(self, v: i128) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::I128(v)))
    }

    fn serialize_u8(self, v: u8) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::U8(v)))
    }

    fn serialize_u16(self, v: u16) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::U16(v)))
    }

    fn serialize_u32(self, v: u32) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::U32(v)))
    }

    fn serialize_u64(self, v: u64) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::U64(v)))
    }

    fn serialize_u128(self, v: u128) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::U128(v)))
    }

    fn serialize_f32(self, v: f32) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::F32(v)))
    }

    fn serialize_f64(self, v: f64) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::F64(v)))
    }

    fn serialize_char(self, v: char) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Str(v.to_string())))
    }

    fn serialize_str(self, v: &str) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Str(v.to_owned())))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Bytes(v.to_vec())))
    }

    fn serialize_none(self) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Null))
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<u32, AdbError> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Null))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Null))
    }

    fn serialize_unit_variant(self, _name: &'static str, _index: u32, variant: &'static str) -> Result<u32, AdbError> {
        Ok(codec::push_leaf(self.buf, &Scalar::Str(variant.to_owned())))
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _name: &'static str, value: &T) -> Result<u32, AdbError> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<u32, AdbError> {
        let payload = value.serialize(BlobSerializer {
            buf: &mut *self.buf
        })?;

        Ok(wrap_variant(self.buf, variant, payload))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder<'b>, AdbError> {
        Ok(SeqBuilder {
            buf:     self.buf,
            offsets: Vec::with_capacity(len.unwrap_or(0)),
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<SeqBuilder<'b>, AdbError> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<SeqBuilder<'b>, AdbError> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<TupleVariantBuilder<'b>, AdbError> {
        Ok(TupleVariantBuilder {
            buf: self.buf,
            variant,
            offsets: Vec::with_capacity(len),
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<MapBuilder<'b>, AdbError> {
        Ok(MapBuilder {
            buf:         self.buf,
            entries:     BTreeMap::new(),
            pending_key: None,
        })
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<StructBuilder<'b>, AdbError> {
        Ok(StructBuilder {
            buf:     self.buf,
            entries: BTreeMap::new(),
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<StructVariantBuilder<'b>, AdbError> {
        Ok(StructVariantBuilder {
            buf: self.buf,
            variant,
            entries: BTreeMap::new(),
        })
    }
}

/// Builds a list vnode — backs sequences, tuples, and tuple structs.
pub(super) struct SeqBuilder<'b> {
    /// The blob being built.
    buf:     &'b mut Vec<u8>,
    /// The offsets of the elements written so far.
    offsets: Vec<u32>,
}

impl SeqBuilder<'_> {
    /// Writes one element into the blob and records its offset.
    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), AdbError> {
        let off = value.serialize(BlobSerializer {
            buf: &mut *self.buf
        })?;
        self.offsets.push(off);

        Ok(())
    }
}

impl SerializeSeq for SeqBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), AdbError> {
        self.element(value)
    }

    fn end(self) -> Result<u32, AdbError> {
        Ok(codec::push_list(self.buf, &self.offsets))
    }
}

impl SerializeTuple for SeqBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), AdbError> {
        self.element(value)
    }

    fn end(self) -> Result<u32, AdbError> {
        Ok(codec::push_list(self.buf, &self.offsets))
    }
}

impl SerializeTupleStruct for SeqBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), AdbError> {
        self.element(value)
    }

    fn end(self) -> Result<u32, AdbError> {
        Ok(codec::push_list(self.buf, &self.offsets))
    }
}

/// Builds `{ variant: [elements] }` for a tuple enum variant.
pub(super) struct TupleVariantBuilder<'b> {
    /// The blob being built.
    buf:     &'b mut Vec<u8>,
    /// The variant name.
    variant: &'static str,
    /// The offsets of the elements written so far.
    offsets: Vec<u32>,
}

impl SerializeTupleVariant for TupleVariantBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), AdbError> {
        let off = value.serialize(BlobSerializer {
            buf: &mut *self.buf
        })?;
        self.offsets.push(off);

        Ok(())
    }

    fn end(self) -> Result<u32, AdbError> {
        let list = codec::push_list(self.buf, &self.offsets);

        Ok(wrap_variant(self.buf, self.variant, list))
    }
}

/// Builds an object vnode from a map, serializing each key to a field name.
pub(super) struct MapBuilder<'b> {
    /// The blob being built.
    buf:         &'b mut Vec<u8>,
    /// The `(field name → child offset)` entries gathered so far.
    entries:     BTreeMap<String, u32>,
    /// The key awaiting its value (set by `serialize_key`).
    pending_key: Option<String>,
}

impl SerializeMap for MapBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), AdbError> {
        self.pending_key = Some(key.serialize(KeySerializer)?);

        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), AdbError> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| AdbError::custom("a map value was serialized before its key"))?;

        let off = value.serialize(BlobSerializer {
            buf: &mut *self.buf
        })?;
        self.entries.insert(key, off);

        Ok(())
    }

    fn end(self) -> Result<u32, AdbError> {
        Ok(finish_object(self.buf, &self.entries))
    }
}

/// Builds an object vnode from a struct's named fields.
pub(super) struct StructBuilder<'b> {
    /// The blob being built.
    buf:     &'b mut Vec<u8>,
    /// The `(field name → child offset)` entries gathered so far.
    entries: BTreeMap<String, u32>,
}

impl SerializeStruct for StructBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), AdbError> {
        let off = value.serialize(BlobSerializer {
            buf: &mut *self.buf
        })?;
        self.entries.insert(key.to_owned(), off);

        Ok(())
    }

    fn end(self) -> Result<u32, AdbError> {
        Ok(finish_object(self.buf, &self.entries))
    }
}

/// Builds `{ variant: { fields } }` for a struct enum variant.
pub(super) struct StructVariantBuilder<'b> {
    /// The blob being built.
    buf:     &'b mut Vec<u8>,
    /// The variant name.
    variant: &'static str,
    /// The `(field name → child offset)` entries gathered so far.
    entries: BTreeMap<String, u32>,
}

impl SerializeStructVariant for StructVariantBuilder<'_> {
    type Error = AdbError;
    type Ok = u32;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), AdbError> {
        let off = value.serialize(BlobSerializer {
            buf: &mut *self.buf
        })?;
        self.entries.insert(key.to_owned(), off);

        Ok(())
    }

    fn end(self) -> Result<u32, AdbError> {
        let object = finish_object(self.buf, &self.entries);

        Ok(wrap_variant(self.buf, self.variant, object))
    }
}

/// Serializes a map key down to a `String` — the object field name it becomes. A
/// string or char is taken as-is; a scalar is stringified; a unit enum variant
/// becomes its name. Anything structural is rejected: an object's keys are names.
struct KeySerializer;

/// Rejects a non-string-like map key with a uniform message.
fn bad_key() -> AdbError {
    AdbError::custom("a map key must serialize to a string or a scalar")
}

impl Serializer for KeySerializer {
    type Error = AdbError;
    type Ok = String;
    type SerializeMap = Impossible<String, AdbError>;
    type SerializeSeq = Impossible<String, AdbError>;
    type SerializeStruct = Impossible<String, AdbError>;
    type SerializeStructVariant = Impossible<String, AdbError>;
    type SerializeTuple = Impossible<String, AdbError>;
    type SerializeTupleStruct = Impossible<String, AdbError>;
    type SerializeTupleVariant = Impossible<String, AdbError>;

    fn serialize_str(self, v: &str) -> Result<String, AdbError> {
        Ok(v.to_owned())
    }

    fn serialize_char(self, v: char) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_bool(self, v: bool) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_i8(self, v: i8) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_i16(self, v: i16) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_i32(self, v: i32) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_i64(self, v: i64) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_i128(self, v: i128) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_u8(self, v: u8) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_u16(self, v: u16) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_u32(self, v: u32) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_u64(self, v: u64) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_u128(self, v: u128) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_f32(self, v: f32) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_f64(self, v: f64) -> Result<String, AdbError> {
        Ok(v.to_string())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<String, AdbError> {
        Ok(variant.to_owned())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<String, AdbError> {
        value.serialize(self)
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<String, AdbError> {
        value.serialize(self)
    }

    fn serialize_bytes(self, _v: &[u8]) -> Result<String, AdbError> {
        Err(bad_key())
    }

    fn serialize_none(self) -> Result<String, AdbError> {
        Err(bad_key())
    }

    fn serialize_unit(self) -> Result<String, AdbError> {
        Err(bad_key())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<String, AdbError> {
        Err(bad_key())
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<String, AdbError> {
        Err(bad_key())
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, AdbError> {
        Err(bad_key())
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, AdbError> {
        Err(bad_key())
    }

    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeTupleStruct, AdbError> {
        Err(bad_key())
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, AdbError> {
        Err(bad_key())
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, AdbError> {
        Err(bad_key())
    }

    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Self::SerializeStruct, AdbError> {
        Err(bad_key())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, AdbError> {
        Err(bad_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_float_key_stringifies() {
        assert_eq!(KeySerializer.serialize_f32(1.5).unwrap(), "1.5");
        assert_eq!(KeySerializer.serialize_f64(2.5).unwrap(), "2.5");
    }

    #[test]
    fn a_structural_key_is_rejected() {
        // Every non-string-like shape a key could take must be refused uniformly.
        assert!(KeySerializer.serialize_bytes(b"x").is_err());
        assert!(KeySerializer.serialize_none().is_err());
        assert!(KeySerializer.serialize_unit().is_err());
        assert!(KeySerializer.serialize_unit_struct("U").is_err());
        assert!(KeySerializer.serialize_newtype_variant("E", 0, "V", &1i32).is_err());
        assert!(KeySerializer.serialize_seq(Some(1)).is_err());
        assert!(KeySerializer.serialize_tuple(2).is_err());
        assert!(KeySerializer.serialize_tuple_struct("T", 2).is_err());
        assert!(KeySerializer.serialize_tuple_variant("E", 0, "V", 2).is_err());
        assert!(KeySerializer.serialize_map(None).is_err());
        assert!(KeySerializer.serialize_struct("S", 1).is_err());
        assert!(KeySerializer.serialize_struct_variant("E", 0, "V", 1).is_err());
    }
}
