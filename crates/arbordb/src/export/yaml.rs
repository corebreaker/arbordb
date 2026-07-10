//! YAML rendering of a [`Value`], in block style.

use super::{scalar::write_scalar, string::quote};
use crate::value::Value;

use std::collections::BTreeMap;

/// Renders `value` as a YAML document (block style), terminated by a newline.
///
/// Scalars are emitted on a single line; a non-empty container opens after its
/// `key:` or `-` marker and indents its contents two spaces per level. Strings
/// are always double-quoted, so a value that would otherwise read as a number, a
/// boolean or `null` keeps its string identity.
pub(super) fn to_yaml(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Node(entries) if !entries.is_empty() => object(&mut out, entries, 0),
        Value::List(items) if !items.is_empty() => list(&mut out, items, 0),
        other => {
            inline(&mut out, other);
            out.push('\n');
        }
    }

    out
}

fn object(out: &mut String, entries: &BTreeMap<String, Value>, indent: usize) {
    for (key, value) in entries {
        pad(out, indent);
        quote(out, key);
        out.push(':');
        child(out, value, indent);
    }
}

fn list(out: &mut String, items: &[Value], indent: usize) {
    for item in items {
        pad(out, indent);
        out.push('-');
        child(out, item, indent);
    }
}

/// Writes the value that follows a `key:` or `-` marker: a non-empty container
/// opens on the next line, indented one level deeper; anything else stays inline
/// after a single space. Either way the entry ends with a newline.
fn child(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Node(entries) if !entries.is_empty() => {
            out.push('\n');
            object(out, entries, indent + 2);
        }
        Value::List(items) if !items.is_empty() => {
            out.push('\n');
            list(out, items, indent + 2);
        }
        other => {
            out.push(' ');
            inline(out, other);
            out.push('\n');
        }
    }
}

/// Writes a value on a single line: a scalar leaf, or an empty container as its
/// flow form (`[]` / `{}`).
fn inline(out: &mut String, value: &Value) {
    match value {
        Value::Leaf(scalar) => write_scalar(out, scalar),
        Value::List(_) => out.push_str("[]"),
        Value::Node(_) => out.push_str("{}"),
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

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Node(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn leaf(scalar: Scalar) -> Value {
        Value::Leaf(scalar)
    }

    #[test]
    fn nested_block_style() {
        let v = obj(vec![
            ("age", leaf(Scalar::U32(30))),
            (
                "tags",
                Value::List(vec![leaf(Scalar::Str("x".into())), leaf(Scalar::Str("y".into()))]),
            ),
        ]);

        assert_eq!(to_yaml(&v), "\"age\": 30\n\"tags\":\n  - \"x\"\n  - \"y\"\n");
    }

    #[test]
    fn list_of_objects() {
        let v = Value::List(vec![obj(vec![("a", leaf(Scalar::I32(1)))])]);

        assert_eq!(to_yaml(&v), "-\n  \"a\": 1\n");
    }

    #[test]
    fn scalars_and_empties() {
        assert_eq!(to_yaml(&leaf(Scalar::Null)), "null\n");
        assert_eq!(to_yaml(&Value::List(vec![])), "[]\n");
        assert_eq!(to_yaml(&obj(vec![("e", Value::Node(BTreeMap::new()))])), "\"e\": {}\n");
    }
}
