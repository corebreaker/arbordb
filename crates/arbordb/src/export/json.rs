//! JSON rendering of a [`Value`].

use super::{scalar::write_scalar, string::quote};
use crate::value::Value;

/// Renders `value` as a JSON document. `indent` selects the layout: `None`
/// yields compact JSON (no whitespace), `Some(n)` pretty-prints it with `n`
/// spaces of indentation per nesting level.
pub(super) fn to_json(value: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    match indent {
        Some(step) => pretty(&mut out, value, step, 0),
        None => compact(&mut out, value),
    }

    out
}

fn compact(out: &mut String, value: &Value) {
    match value {
        Value::Leaf(scalar) => write_scalar(out, scalar),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }

                compact(out, item);
            }
            out.push(']');
        }
        Value::Node(entries) => {
            out.push('{');
            for (i, (key, val)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }

                quote(out, key);
                out.push(':');
                compact(out, val);
            }
            out.push('}');
        }
    }
}

fn pretty(out: &mut String, value: &Value, step: usize, depth: usize) {
    match value {
        Value::List(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }

                pad(out, step * (depth + 1));
                pretty(out, item, step, depth + 1);
            }
            out.push('\n');
            pad(out, step * depth);
            out.push(']');
        }
        Value::Node(entries) if !entries.is_empty() => {
            out.push_str("{\n");
            for (i, (key, val)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }

                pad(out, step * (depth + 1));
                quote(out, key);
                out.push_str(": ");
                pretty(out, val, step, depth + 1);
            }
            out.push('\n');
            pad(out, step * depth);
            out.push('}');
        }
        Value::List(_) => out.push_str("[]"),
        Value::Node(_) => out.push_str("{}"),
        Value::Leaf(scalar) => write_scalar(out, scalar),
    }
}

fn pad(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Scalar;

    use std::collections::BTreeMap;

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Node(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn leaf(scalar: Scalar) -> Value {
        Value::Leaf(scalar)
    }

    #[test]
    fn compact_object_is_sorted_and_minimal() {
        let v = obj(vec![
            ("name", leaf(Scalar::Str("a\"b".into()))),
            ("age", leaf(Scalar::I32(3))),
        ]);

        assert_eq!(to_json(&v, None), r#"{"age":3,"name":"a\"b"}"#);
    }

    #[test]
    fn pretty_indents_by_step() {
        let v = obj(vec![(
            "xs",
            Value::List(vec![leaf(Scalar::I32(1)), leaf(Scalar::Bool(true))]),
        )]);

        assert_eq!(to_json(&v, Some(2)), "{\n  \"xs\": [\n    1,\n    true\n  ]\n}");
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        let v = obj(vec![("a", Value::List(vec![])), ("b", Value::Node(BTreeMap::new()))]);

        assert_eq!(to_json(&v, Some(2)), "{\n  \"a\": [],\n  \"b\": {}\n}");
    }

    #[test]
    fn top_level_leaf() {
        assert_eq!(to_json(&leaf(Scalar::Null), None), "null");
        assert_eq!(to_json(&leaf(Scalar::Null), Some(2)), "null");
    }
}
