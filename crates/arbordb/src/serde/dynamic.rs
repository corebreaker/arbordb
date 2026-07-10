//! Serde for the dynamic [`Value`] and its [`Scalar`] leaves.
//!
//! The representation is *structural*, exactly like every other dynamic value type
//! (`serde_json::Value`, `toml::Value`, …): a leaf serializes as its bare scalar, a
//! list as a sequence, and an object as a map — no variant tags. So a `Value`
//! serializes to clean, idiomatic JSON/YAML/… and reconstructs from any
//! self-describing format.
//!
//! Round-trip caveats follow from that generality (they match `serde_json::Value`):
//! the scalars a self-describing format cannot distinguish are normalized on the way
//! back — a JSON integer returns as [`U64`](Scalar::U64) or [`I64`](Scalar::I64),
//! a float as [`F64`](Scalar::F64) — and the scalar types with no Serde primitive
//! (uuid, date/time, duration, big numbers) serialize as strings and return as
//! [`Str`](Scalar::Str). Width-preserving self-describing formats (CBOR, MessagePack)
//! keep the exact integer width.

use crate::{data::Scalar, value::Value};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};

use std::{collections::BTreeMap, fmt};

impl Serialize for Scalar {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Scalar::Null => serializer.serialize_unit(),
            Scalar::Bool(v) => serializer.serialize_bool(*v),
            Scalar::I8(v) => serializer.serialize_i8(*v),
            Scalar::I16(v) => serializer.serialize_i16(*v),
            Scalar::I32(v) => serializer.serialize_i32(*v),
            Scalar::I64(v) => serializer.serialize_i64(*v),
            Scalar::I128(v) => serializer.serialize_i128(*v),
            Scalar::U8(v) => serializer.serialize_u8(*v),
            Scalar::U16(v) => serializer.serialize_u16(*v),
            Scalar::U32(v) => serializer.serialize_u32(*v),
            Scalar::U64(v) => serializer.serialize_u64(*v),
            Scalar::U128(v) => serializer.serialize_u128(*v),
            Scalar::F32(v) => serializer.serialize_f32(*v),
            Scalar::F64(v) => serializer.serialize_f64(*v),
            Scalar::Str(v) => serializer.serialize_str(v),
            Scalar::Bytes(v) => serializer.serialize_bytes(v),

            // The scalar types with no Serde primitive serialize as their canonical
            // string form (and come back as `Str` — see the module docs).
            Scalar::Uuid(v) => serializer.serialize_str(&v.to_string()),
            Scalar::Date(v) => serializer.serialize_str(&v.to_string()),
            Scalar::Time(v) => serializer.serialize_str(&v.to_string()),
            Scalar::DateTime(v) => serializer.serialize_str(&v.to_rfc3339()),
            Scalar::Duration(v) => serializer.serialize_str(&v.to_string()),

            #[cfg(feature = "bigint-as-scalar")]
            Scalar::BigInt(v) => serializer.serialize_str(&v.to_string()),
            #[cfg(feature = "bigfloat-as-scalar")]
            Scalar::BigFloat(v) => serializer.serialize_str(&v.to_string()),
            #[cfg(feature = "rational-as-scalar")]
            Scalar::Rational(v) => serializer.serialize_str(&v.to_string()),
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Leaf(scalar) => scalar.serialize(serializer),
            Value::List(items) => items.serialize(serializer),
            Value::Node(map) => map.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Leaf(scalar) => Ok(scalar),
            Value::List(_) => Err(de::Error::custom("expected a scalar, found a sequence")),
            Value::Node(_) => Err(de::Error::custom("expected a scalar, found a map")),
        }
    }
}

/// Builds a [`Value`] from any self-describing Serde input, mapping each primitive
/// to the matching [`Scalar`], a sequence to a [`List`](Value::List), and a map to a
/// [`Node`](Value::Node).
struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any ArborDb value (a scalar, a sequence, or a map)")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Bool(v)))
    }

    fn visit_i8<E: de::Error>(self, v: i8) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::I8(v)))
    }

    fn visit_i16<E: de::Error>(self, v: i16) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::I16(v)))
    }

    fn visit_i32<E: de::Error>(self, v: i32) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::I32(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::I64(v)))
    }

    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::I128(v)))
    }

    fn visit_u8<E: de::Error>(self, v: u8) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::U8(v)))
    }

    fn visit_u16<E: de::Error>(self, v: u16) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::U16(v)))
    }

    fn visit_u32<E: de::Error>(self, v: u32) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::U32(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::U64(v)))
    }

    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::U128(v)))
    }

    fn visit_f32<E: de::Error>(self, v: f32) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::F32(v)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::F64(v)))
    }

    fn visit_char<E: de::Error>(self, v: char) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Str(v.to_string())))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Str(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Str(v)))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Bytes(v.to_vec())))
    }

    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Bytes(v)))
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Null))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Leaf(Scalar::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }

        Ok(Value::List(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut node = BTreeMap::new();
        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            node.insert(key, value);
        }

        Ok(Value::Node(node))
    }
}

#[cfg(test)]
mod tests {
    use crate::{data::Scalar, value::Value};

    use std::collections::BTreeMap;

    fn sample() -> Value {
        Value::Node(BTreeMap::from([
            (String::from("n"), Value::Leaf(Scalar::U64(42))),
            (
                String::from("tags"),
                Value::List(vec![
                    Value::Leaf(Scalar::Str(String::from("a"))),
                    Value::Leaf(Scalar::Bool(true)),
                ]),
            ),
            (String::from("nil"), Value::Leaf(Scalar::Null)),
        ]))
    }

    #[test]
    fn value_serializes_structurally_without_variant_tags() {
        let json = serde_json::to_string(&sample()).unwrap();

        // Bare, idiomatic JSON — keys are sorted (a `Node` is a `BTreeMap`).
        assert_eq!(json, r#"{"n":42,"nil":null,"tags":["a",true]}"#);
    }

    #[test]
    fn value_round_trips_through_json() {
        let json = serde_json::to_string(&sample()).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(back, sample());
    }

    #[test]
    fn scalars_map_to_the_expected_json_kinds() {
        // A bare scalar deserializes on its own; JSON normalizes numbers by sign.
        assert_eq!(serde_json::from_str::<Scalar>("7").unwrap(), Scalar::U64(7));
        assert_eq!(serde_json::from_str::<Scalar>("-3").unwrap(), Scalar::I64(-3));
        assert_eq!(serde_json::from_str::<Scalar>("1.5").unwrap(), Scalar::F64(1.5));
        assert_eq!(serde_json::from_str::<Scalar>("null").unwrap(), Scalar::Null);

        // A non-scalar is rejected when the target is a `Scalar`.
        assert!(serde_json::from_str::<Scalar>("[1,2]").is_err());
    }
}
