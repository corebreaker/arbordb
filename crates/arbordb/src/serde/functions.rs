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
}
