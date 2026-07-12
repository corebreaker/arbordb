//! Reading through a write transaction.
//!
//! A [`WriteTxn`](arbordb::txn::WriteTxn) exposes the same reads as a
//! [`ReadTxn`](arbordb::txn::ReadTxn), but over its **own uncommitted state** — so
//! a read-modify-write stays atomic within one transaction, with no window for a
//! concurrent commit to slip between a separate read and the write.

use arbordb::{data::Scalar, entry::EntryKind, export::JsonExporter, ArborDb, Value};

use std::collections::BTreeMap;

fn user(age: i64) -> Value {
    Value::Node(BTreeMap::from([(String::from("age"), Value::Leaf(Scalar::I64(age)))]))
}

#[test]
fn a_writer_reads_its_own_uncommitted_state() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store_value("users/alice", &user(30)).unwrap();

    // Every read surface reflects the pending write, before any commit.
    assert!(w.exists("users/alice").unwrap());
    assert_eq!(w.kind("users/alice").unwrap(), Some(EntryKind::File));
    assert_eq!(w.get_as::<i64>("users/alice", "age").unwrap(), Some(30));
    assert_eq!(w.load_value("users/alice").unwrap(), Some(user(30)));

    let names: Vec<String> = w
        .ls("users")
        .unwrap()
        .into_iter()
        .map(|e| e.name().to_string())
        .collect();
    assert_eq!(names, vec![String::from("alice")]);

    w.commit().unwrap();

    let r = table.read().unwrap();
    assert_eq!(r.get_as::<i64>("users/alice", "age").unwrap(), Some(30));
}

#[test]
fn read_modify_write_is_atomic_in_one_transaction() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store_value("counter/n", &user(1)).unwrap();
        w.commit().unwrap();
    }

    // Increment without a separate read transaction: the read and the write share
    // one snapshot, so nothing can slip in between them.
    {
        let w = table.write().unwrap();
        let current = w.get_as::<i64>("counter/n", "age").unwrap().unwrap();
        w.store_value("counter/n", &user(current + 1)).unwrap();

        // The follow-up read already reflects the update.
        assert_eq!(w.get_as::<i64>("counter/n", "age").unwrap(), Some(current + 1));

        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.get_as::<i64>("counter/n", "age").unwrap(), Some(2));
}

#[test]
fn an_uncommitted_write_is_discarded_on_rollback() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store_value("users/ghost", &user(99)).unwrap();

        // The writer sees its own pending value...
        assert_eq!(w.get_as::<i64>("users/ghost", "age").unwrap(), Some(99));

        // ...but the transaction is dropped without committing.
    }

    // A reader never sees the rolled-back write.
    let r = table.read().unwrap();
    assert_eq!(r.get_as::<i64>("users/ghost", "age").unwrap(), None);
    assert!(!r.exists("users/ghost").unwrap());
}

#[test]
fn a_writer_exports_its_own_uncommitted_value() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store_value("greeting", &Value::Leaf(Scalar::Str(String::from("hello"))))
        .unwrap();

    assert_eq!(w.export_to_json("greeting", None).unwrap(), "\"hello\"");

    w.commit().unwrap();
}

#[test]
fn rooted_write_reads_relative_paths() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    let users = w.rooted("users").unwrap();
    users.store_value("alice", &user(30)).unwrap();

    // The rooted write view reads its own uncommitted state, relative to the root.
    assert_eq!(users.get_as::<i64>("alice", "age").unwrap(), Some(30));
    assert!(users.exists("alice").unwrap());
    assert_eq!(users.load_value("alice").unwrap(), Some(user(30)));

    w.commit().unwrap();
}

/// The index query surface works over a writer's uncommitted entries too.
#[cfg(feature = "derive")]
mod indexed {
    use arbordb::{
        data::Scalar,
        index::{IndexColumn, IndexDef},
        path::VPath,
        AData,
        ArborDb,
    };

    #[derive(AData, Debug, Clone, PartialEq)]
    struct Member {
        handle: String,
        age:    i64,
    }

    fn member(handle: &str, age: i64) -> Member {
        Member {
            handle: handle.into(),
            age,
        }
    }

    fn by_age() -> IndexDef {
        IndexDef::new(
            String::from("by_age"),
            String::from("members/*"),
            vec![IndexColumn::asc(VPath::root().child_name("age"))],
            false,
        )
    }

    #[test]
    fn a_writer_queries_its_own_uncommitted_index_entries() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();
        table.create_index(&by_age()).unwrap();

        let w = table.write().unwrap();
        w.store("members/alice", &member("alice", 30)).unwrap();
        w.store("members/bob", &member("bob", 40)).unwrap();

        // The index is maintained in this same transaction, so the writer's query
        // sees the not-yet-committed entities.
        let hits: Vec<Member> = w.find("by_age", &[Scalar::I64(30)]).unwrap();
        assert_eq!(hits, vec![member("alice", 30)]);

        let all: Vec<Member> = w.query("by_age").run().unwrap();
        assert_eq!(all, vec![member("alice", 30), member("bob", 40)]);

        w.commit().unwrap();
    }
}
