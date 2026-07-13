//! The Serde entry points: `to_blob` / `from_blob` go straight to and from the value
//! codec; `to_value` / `from_value` bridge through an owned [`Value`] on top of them.

use super::{blob_de, blob_ser};
use crate::{codec, error::AdbResult, value::Value};
use serde::{de::DeserializeOwned, Serialize};

/// Serializes `value` straight into a value blob — no intermediate [`Value`] tree.
pub(crate) fn to_blob<T: Serialize + ?Sized>(value: &T) -> AdbResult<Vec<u8>> {
    blob_ser::to_blob(value)
}

/// Reconstructs a `T` straight from a value blob, reading it zero-copy — no
/// intermediate [`Value`] tree.
pub(crate) fn from_blob<T: DeserializeOwned>(blob: &[u8]) -> AdbResult<T> {
    blob_de::from_blob(blob)
}

/// Maps `value` into an owned [`Value`] tree (through the blob codec).
pub(crate) fn to_value<T: Serialize + ?Sized>(value: &T) -> AdbResult<Value> {
    codec::decode(&to_blob(value)?)
}

/// Reconstructs a `T` from an owned [`Value`] tree (through the blob codec).
pub(crate) fn from_value<T: DeserializeOwned>(value: &Value) -> AdbResult<T> {
    from_blob(&codec::encode(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{data::Scalar, value::Value};

    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Inner {
        flag: bool,
        tags: Vec<String>,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    enum Shape {
        Unit,
        Newtype(i64),
        Tuple(i64, i64),
        Struct { width: u32, label: String },
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Record {
        id:     u64,
        name:   String,
        score:  f64,
        note:   Option<String>,
        inner:  Inner,
        shape:  Shape,
        counts: BTreeMap<String, i64>,
    }

    fn sample() -> Record {
        Record {
            id:     7,
            name:   String::from("alice"),
            score:  1.5,
            note:   None,
            inner:  Inner {
                flag: true,
                tags: vec![String::from("a"), String::from("b")],
            },
            shape:  Shape::Struct {
                width: 3,
                label: String::from("box"),
            },
            counts: BTreeMap::from([(String::from("x"), 1), (String::from("y"), 2)]),
        }
    }

    #[test]
    fn to_blob_round_trips_directly() {
        let blob = to_blob(&sample()).unwrap();
        let back: Record = from_blob(&blob).unwrap();

        assert_eq!(back, sample());
    }

    #[test]
    fn blob_and_value_paths_agree() {
        // The direct blob is a valid value blob: decoding it yields the same tree the
        // `Value` bridge builds, and `from_value` reads a blob built by the codec.
        let via_blob = codec::decode(&to_blob(&sample()).unwrap()).unwrap();
        let via_value = to_value(&sample()).unwrap();
        assert_eq!(via_blob, via_value);

        let back: Record = from_value(&via_value).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn round_trips_a_nested_record() {
        let value = to_value(&sample()).unwrap();

        // It is stored as a native object, not an opaque blob.
        assert!(matches!(value, Value::Node(_)));
        assert_eq!(
            value.get_value("name"),
            Some(Value::Leaf(Scalar::Str(String::from("alice"))))
        );
        assert_eq!(value.get_value("inner/flag"), Some(Value::Leaf(Scalar::Bool(true))));

        let back: Record = from_value(&value).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn round_trips_each_enum_shape() {
        for shape in [Shape::Unit, Shape::Newtype(9), Shape::Tuple(1, 2)] {
            let blob = to_blob(&shape).unwrap();
            let back: Shape = from_blob(&blob).unwrap();
            assert_eq!(back, shape);
        }

        // A unit variant is stored as its bare name.
        assert_eq!(
            to_value(&Shape::Unit).unwrap(),
            Value::Leaf(Scalar::Str(String::from("Unit")))
        );
    }

    /// Every native scalar leaf survives the blob round trip with its exact type kept —
    /// unlike a self-describing format, the codec preserves the width. This drives one
    /// `serialize_*` / `visit_*` pair per scalar across the whole Serde bridge at once.
    #[test]
    fn every_scalar_leaf_round_trips_through_the_blob() {
        let leaves = [
            Scalar::Null,
            Scalar::Bool(true),
            Scalar::I8(-8),
            Scalar::I16(-16),
            Scalar::I32(-32),
            Scalar::I64(-64),
            Scalar::I128(-128),
            Scalar::U8(8),
            Scalar::U16(16),
            Scalar::U32(32),
            Scalar::U64(64),
            Scalar::U128(128),
            Scalar::F32(1.5),
            Scalar::F64(2.5),
            Scalar::Str(String::from("s")),
            Scalar::Bytes(vec![1, 2, 3]),
        ];

        for leaf in leaves {
            let value = Value::Leaf(leaf.clone());
            let back: Value = from_blob(&to_blob(&value).unwrap()).unwrap();

            assert_eq!(back, value, "scalar {leaf:?} did not round-trip");
        }
    }

    /// A unit struct, used to drive `serialize_unit_struct`.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct UnitStruct;

    /// A newtype struct, used to drive `serialize_newtype_struct`.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct NewtypeStruct(i32);

    /// A tuple struct, used to drive `serialize_tuple_struct`.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TupleStruct(i32, i32);

    /// An enum exercising all four variant shapes in one round trip.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    enum Every {
        Unit,
        Newtype(i64),
        Tuple(i64, i64),
        Struct { a: u32, b: String },
    }

    /// A record touching every scalar width and every container the serializer builds:
    /// options (some and none), a unit, a unit/newtype/tuple struct, a sequence, a
    /// tuple, a map, and each enum variant shape.
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Everything {
        b:            bool,
        i8v:          i8,
        i16v:         i16,
        i32v:         i32,
        i64v:         i64,
        i128v:        i128,
        u8v:          u8,
        u16v:         u16,
        u32v:         u32,
        u64v:         u64,
        u128v:        u128,
        f32v:         f32,
        f64v:         f64,
        ch:           char,
        s:            String,
        some:         Option<i32>,
        none:         Option<i32>,
        unit:         (),
        unit_struct:  UnitStruct,
        newtype:      NewtypeStruct,
        seq:          Vec<i32>,
        tuple:        (i32, i32),
        tuple_struct: TupleStruct,
        map:          BTreeMap<String, i32>,
        e_unit:       Every,
        e_newtype:    Every,
        e_tuple:      Every,
        e_struct:     Every,
    }

    #[test]
    fn a_record_with_every_shape_round_trips() {
        let value = Everything {
            b:            true,
            i8v:          -8,
            i16v:         -16,
            i32v:         -32,
            i64v:         -64,
            i128v:        -128,
            u8v:          8,
            u16v:         16,
            u32v:         32,
            u64v:         64,
            u128v:        128,
            f32v:         1.5,
            f64v:         2.5,
            ch:           'q',
            s:            String::from("hi"),
            some:         Some(1),
            none:         None,
            unit:         (),
            unit_struct:  UnitStruct,
            newtype:      NewtypeStruct(3),
            seq:          vec![1, 2, 3],
            tuple:        (4, 5),
            tuple_struct: TupleStruct(6, 7),
            map:          BTreeMap::from([(String::from("k"), 9)]),
            e_unit:       Every::Unit,
            e_newtype:    Every::Newtype(10),
            e_tuple:      Every::Tuple(11, 12),
            e_struct:     Every::Struct {
                a: 13,
                b: String::from("v"),
            },
        };

        let back: Everything = from_blob(&to_blob(&value).unwrap()).unwrap();
        assert_eq!(back, value);
    }

    /// A newtype-struct map key, exercising `KeySerializer::serialize_newtype_struct`.
    #[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Debug)]
    struct KeyId(u32);

    /// A unit-variant enum used as a map key, exercising the enum key path.
    #[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Debug)]
    enum KeyTag {
        Alpha,
        Beta,
    }

    /// Round-trips a one-entry map keyed by `key`, so the key type drives both
    /// `KeySerializer` (stringifying the key) and `MapKeyDeserializer` (parsing it back).
    fn key_map_round_trips<K>(key: K)
    where
        K: serde::Serialize + serde::de::DeserializeOwned + Ord + std::fmt::Debug, {
        let map = BTreeMap::from([(key, 5i32)]);
        let back: BTreeMap<K, i32> = from_blob(&to_blob(&map).unwrap()).unwrap();

        assert_eq!(back, map);
    }

    #[test]
    fn typed_map_keys_round_trip() {
        key_map_round_trips(true);
        key_map_round_trips(-1i8);
        key_map_round_trips(-2i16);
        key_map_round_trips(-3i32);
        key_map_round_trips(-4i64);
        key_map_round_trips(-5i128);
        key_map_round_trips(6u8);
        key_map_round_trips(7u16);
        key_map_round_trips(8u32);
        key_map_round_trips(9u64);
        key_map_round_trips(10u128);
        key_map_round_trips('c');
        key_map_round_trips(KeyId(11));
        key_map_round_trips(KeyTag::Alpha);
        key_map_round_trips(Some(12u32));
        let _ = KeyTag::Beta;
    }
}
