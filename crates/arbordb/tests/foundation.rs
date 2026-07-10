//! Foundation storage tests: the virtual-filesystem API and value round-trips.

use arbordb::{data::Scalar, ArborDb, EntryKind, Value};
use std::collections::BTreeMap;

/// A small `{ name, age }` document.
fn user(name: &str, age: u32) -> Value {
    let mut fields = BTreeMap::new();
    fields.insert(String::from("name"), Value::Leaf(Scalar::Str(name.to_string())));
    fields.insert(String::from("age"), Value::Leaf(Scalar::U32(age)));

    Value::Node(fields)
}

#[test]
fn stores_and_loads_a_value() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store("users/alice", &user("Alice", 30)).unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();
    assert_eq!(r.load("users/alice").unwrap(), Some(user("Alice", 30)));
    assert_eq!(
        r.get("users/alice", "name").unwrap(),
        Some(Scalar::Str(String::from("Alice")))
    );
    assert_eq!(r.get_as::<u32>("users/alice", "age").unwrap(), Some(30));
    assert_eq!(r.load("users/bob").unwrap(), None);
}

#[test]
fn reports_kinds_and_lists_a_directory() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store("users/alice", &user("Alice", 30)).unwrap();
    w.store("users/bob", &user("Bob", 40)).unwrap();
    w.mkdir("users/teams").unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();
    assert_eq!(r.kind("users").unwrap(), Some(EntryKind::Dir));
    assert_eq!(r.kind("users/alice").unwrap(), Some(EntryKind::File));
    assert_eq!(r.kind("users/teams").unwrap(), Some(EntryKind::Dir));
    assert_eq!(r.kind("missing").unwrap(), None);

    // `ls` comes back in name order.
    assert_eq!(
        r.ls("users").unwrap(),
        vec![
            (String::from("alice"), EntryKind::File),
            (String::from("bob"), EntryKind::File),
            (String::from("teams"), EntryKind::Dir),
        ]
    );
}

#[test]
fn navigates_nested_values_by_vpath() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let mut address = BTreeMap::new();
    address.insert(String::from("city"), Value::Leaf(Scalar::Str(String::from("Paris"))));

    let mut doc = BTreeMap::new();
    doc.insert(String::from("address"), Value::Node(address));
    doc.insert(
        String::from("scores"),
        Value::List(vec![Value::Leaf(Scalar::I64(10)), Value::Leaf(Scalar::I64(20))]),
    );

    let w = table.write().unwrap();
    w.store("p", &Value::Node(doc)).unwrap();
    w.commit().unwrap();

    let r = table.read().unwrap();
    assert_eq!(
        r.get("p", "address/city").unwrap(),
        Some(Scalar::Str(String::from("Paris")))
    );
    assert_eq!(r.get_as::<i64>("p", "scores[1]").unwrap(), Some(20));
    assert_eq!(r.get("p", "scores[5]").unwrap(), None); // out of range
    assert_eq!(r.get("p", "address").unwrap(), None); // not a scalar leaf
}

#[test]
fn overwrites_a_file_then_removes_it() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store("a/x", &user("X", 1)).unwrap();
        w.commit().unwrap();
    }
    {
        let w = table.write().unwrap();
        w.store("a/x", &user("X", 2)).unwrap();
        w.commit().unwrap();
    }

    {
        let r = table.read().unwrap();
        assert_eq!(r.get_as::<u32>("a/x", "age").unwrap(), Some(2));
    }

    {
        let w = table.write().unwrap();
        assert!(w.rm("a/x").unwrap());
        assert!(!w.rm("a/x").unwrap()); // already gone
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load("a/x").unwrap(), None);
    assert!(r.exists("a").unwrap()); // the parent directory survives
}

#[test]
fn removing_a_directory_cascades() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store("org/team/alice", &user("Alice", 30)).unwrap();
    w.store("org/team/bob", &user("Bob", 40)).unwrap();
    w.commit().unwrap();

    let w = table.write().unwrap();
    assert!(w.rm("org/team").unwrap());
    w.commit().unwrap();

    let r = table.read().unwrap();
    assert_eq!(r.load("org/team/alice").unwrap(), None);
    assert!(!r.exists("org/team").unwrap());
    assert!(r.exists("org").unwrap());
}

#[test]
fn persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.arbordb");

    {
        let db = ArborDb::create(&path).unwrap();
        let table = db.open_table("t").unwrap();
        let w = table.write().unwrap();
        w.store("users/alice", &user("Alice", 30)).unwrap();
        w.commit().unwrap();
    }

    {
        let db = ArborDb::open(&path).unwrap();
        let table = db.open_table("t").unwrap();
        let r = table.read().unwrap();
        assert_eq!(r.load("users/alice").unwrap(), Some(user("Alice", 30)));
    }
}

#[test]
fn rejects_reserved_and_empty_table_names() {
    let db = ArborDb::create_in_memory().unwrap();

    assert!(db.open_table("").is_err());
    assert!(db.open_table("$metadata").is_err());
    assert!(db.open_table("ok").is_ok());
}
