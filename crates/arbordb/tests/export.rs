//! Integration tests for JSON and YAML export of stored values and in-memory
//! [`Value`] subtrees.

use arbordb::{
    data::Scalar,
    export::{JsonExporter, YamlExporter},
    AdbError,
    ArborDb,
    Value,
};

fn mem_db() -> ArborDb {
    ArborDb::create_in_memory().expect("create db")
}

fn node(pairs: Vec<(&str, Value)>) -> Value {
    Value::Node(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

/// A record exercising every export-relevant shape: booleans, integers, floats,
/// null, a string, raw bytes, and a nested list.
fn sample() -> Value {
    node(vec![
        ("active", Value::Leaf(Scalar::Bool(true))),
        ("age", Value::Leaf(Scalar::U32(30))),
        ("avatar", Value::Leaf(Scalar::Bytes(vec![0, 1, 2, 255]))),
        ("name", Value::Leaf(Scalar::Str(String::from("Alice")))),
        ("nickname", Value::Leaf(Scalar::Null)),
        ("score", Value::Leaf(Scalar::F64(1.5))),
        (
            "tags",
            Value::List(vec![
                Value::Leaf(Scalar::Str(String::from("x"))),
                Value::Leaf(Scalar::Str(String::from("y"))),
            ]),
        ),
    ])
}

#[test]
fn exports_a_stored_file_value() {
    let db = mem_db();
    let table = db.open_table("data").unwrap();

    let w = table.write().unwrap();
    w.store_value("user", &sample()).unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();

    let compact = concat!(
        r#"{"active":true,"age":30,"avatar":"AAEC/w==","#,
        r#""name":"Alice","nickname":null,"score":1.5,"tags":["x","y"]}"#,
    );

    assert_eq!(r.export_to_json("user", None).unwrap(), compact);

    let pretty = [
        "{",
        "  \"active\": true,",
        "  \"age\": 30,",
        "  \"avatar\": \"AAEC/w==\",",
        "  \"name\": \"Alice\",",
        "  \"nickname\": null,",
        "  \"score\": 1.5,",
        "  \"tags\": [",
        "    \"x\",",
        "    \"y\"",
        "  ]",
        "}",
    ]
    .join("\n");

    assert_eq!(r.export_to_json("user", Some(2)).unwrap(), pretty);

    let yaml = [
        "\"active\": true",
        "\"age\": 30",
        "\"avatar\": \"AAEC/w==\"",
        "\"name\": \"Alice\"",
        "\"nickname\": null",
        "\"score\": 1.5",
        "\"tags\":",
        "  - \"x\"",
        "  - \"y\"",
        "",
    ]
    .join("\n");

    assert_eq!(r.export_to_yaml("user").unwrap(), yaml);
}

#[test]
fn exports_a_scalar_and_a_list_file() {
    let db = mem_db();
    let table = db.open_table("data").unwrap();

    let w = table.write().unwrap();
    w.store_value("greeting", &Value::Leaf(Scalar::Str(String::from("hello"))))
        .unwrap();
    w.store_value(
        "nums",
        &Value::List(vec![Value::Leaf(Scalar::U32(1)), Value::Leaf(Scalar::U32(2))]),
    )
    .unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();

    // A single scalar leaf.
    assert_eq!(r.export_to_json("greeting", None).unwrap(), "\"hello\"");
    assert_eq!(r.export_to_yaml("greeting").unwrap(), "\"hello\"\n");

    // A list, pretty and as YAML block style.
    assert_eq!(r.export_to_json("nums", Some(2)).unwrap(), "[\n  1,\n  2\n]");
    assert_eq!(r.export_to_yaml("nums").unwrap(), "- 1\n- 2\n");
}

#[test]
fn a_directory_cannot_be_exported() {
    let db = mem_db();
    let table = db.open_table("data").unwrap();

    // Storing a file under "dir" makes "dir" a directory.
    let w = table.write().unwrap();
    w.store_value("dir/child", &Value::Leaf(Scalar::I64(1))).unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();

    assert!(matches!(
        r.export_to_json("dir", None).unwrap_err(),
        AdbError::CannotAccess(_),
    ));

    assert!(matches!(
        r.export_to_yaml("dir").unwrap_err(),
        AdbError::CannotAccess(_)
    ));

    // The file under it exports normally.
    assert_eq!(r.export_to_json("dir/child", None).unwrap(), "1");
}

#[test]
fn a_missing_path_is_not_found() {
    let db = mem_db();
    let table = db.open_table("data").unwrap();

    let w = table.write().unwrap();
    w.store_value("user", &Value::Leaf(Scalar::I64(1))).unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();

    assert!(matches!(
        r.export_to_json("ghost", None).unwrap_err(),
        AdbError::ValueNotFound(_),
    ));

    assert!(matches!(
        r.export_to_yaml("ghost").unwrap_err(),
        AdbError::ValueNotFound(_)
    ));
}

#[test]
fn a_rooted_view_forwards_export() {
    let db = mem_db();
    let table = db.open_table("data").unwrap();

    let w = table.write().unwrap();
    w.store_value("users/alice", &sample()).unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();
    let rooted = r.rooted("users").unwrap();

    // A path relative to the view's root resolves to "users/alice".
    assert_eq!(
        rooted.export_to_json("alice", None).unwrap(),
        r.export_to_json("users/alice", None).unwrap(),
    );
    assert_eq!(
        rooted.export_to_yaml("alice").unwrap(),
        r.export_to_yaml("users/alice").unwrap(),
    );

    // A missing relative path is still not found.
    assert!(matches!(
        rooted.export_to_json("ghost", None).unwrap_err(),
        AdbError::ValueNotFound(_),
    ));
}

#[test]
fn an_in_memory_value_exports_itself_and_its_subtrees() {
    let value = sample();

    // The whole value at the root.
    assert_eq!(
        value.export_to_json("", None).unwrap(),
        concat!(
            r#"{"active":true,"age":30,"avatar":"AAEC/w==","#,
            r#""name":"Alice","nickname":null,"score":1.5,"tags":["x","y"]}"#,
        ),
    );

    // A navigated subtree: an object field that is a list, a single leaf, and a
    // list element reached by index.
    assert_eq!(value.export_to_json("tags", None).unwrap(), r#"["x","y"]"#);
    assert_eq!(value.export_to_json("name", None).unwrap(), "\"Alice\"");
    assert_eq!(value.export_to_json("tags[0]", None).unwrap(), "\"x\"");
    assert_eq!(value.export_to_yaml("name").unwrap(), "\"Alice\"\n");

    // A segment that leads nowhere.
    assert!(matches!(
        value.export_to_json("ghost", None).unwrap_err(),
        AdbError::PathNotFound(_),
    ));
}
