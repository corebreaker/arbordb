//! Rooted transaction views: paths relative to a fixed root, and root-scoped queries.

use arbordb::{data::Scalar, ArborDb, Value};
use std::collections::BTreeMap;

fn user(age: i64) -> Value {
    Value::Node(BTreeMap::from([(String::from("age"), Value::Leaf(Scalar::I64(age)))]))
}

#[test]
fn rooted_read_and_write_forward_relative_paths() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        let users = w.rooted("users").unwrap();

        users.store_value("alice", &user(30)).unwrap();
        users.store_value("bob", &user(40)).unwrap();
        users.mv("bob", "carol").unwrap(); // users/bob -> users/carol

        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    let users = r.rooted("users").unwrap();

    assert_eq!(users.root().to_string(), "users");
    assert_eq!(users.get_as::<i64>("alice", "age").unwrap(), Some(30));
    assert!(!users.exists("bob").unwrap());
    assert!(users.exists("carol").unwrap());

    // `ls("")` lists the view's own root, in name order.
    let names: Vec<String> = users
        .ls("")
        .unwrap()
        .into_iter()
        .map(|entry| entry.name().to_string())
        .collect();

    assert_eq!(names, ["alice", "carol"]);

    // A nested view descends further; the empty path addresses the nested root itself.
    assert_eq!(
        users.rooted("alice").unwrap().get_as::<i64>("", "age").unwrap(),
        Some(30)
    );

    // The absolute transaction confirms the joined paths.
    assert_eq!(r.get_as::<i64>("users/carol", "age").unwrap(), Some(40));
}

#[test]
fn rooted_write_forwards_every_operation() {
    use arbordb::{
        data::{Leaf, LeafMut},
        entry::EntryKind,
        export::{JsonExporter, YamlExporter},
    };

    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        let root = w.rooted("data").unwrap();
        assert_eq!(root.root().to_string(), "data");

        // A nested view is rooted relative to this one.
        let nested = root.rooted("inner").unwrap();
        assert_eq!(nested.root().to_string(), "data/inner");

        root.store("n", &5i64).unwrap();
        root.store_value("v", &user(30)).unwrap();
        root.mkdir("dir").unwrap();

        assert!(root.exists("dir").unwrap());
        assert_eq!(root.kind("dir").unwrap(), Some(EntryKind::Dir));
        assert_eq!(root.kind("n").unwrap(), Some(EntryKind::File));

        {
            let leaf: LeafMut<'_, i64> = root.fetch_mut("n").unwrap().unwrap();
            leaf.set(&6).unwrap();
        }

        assert_eq!(root.load::<i64>("n").unwrap(), Some(6));
        assert_eq!(root.load_value("v").unwrap(), Some(user(30)));
        assert_eq!(root.get_as::<i64>("v", "age").unwrap(), Some(30));
        assert!(root.get("v", "age").unwrap().is_some());

        {
            let leaf: Leaf<'static, i64> = root.fetch("n").unwrap().unwrap();
            assert_eq!(leaf.get().unwrap(), 6);
        }

        let names: Vec<String> = root.ls("").unwrap().into_iter().map(|e| e.name().to_string()).collect();
        assert!(names.contains(&String::from("n")));

        root.cp("v", "v2").unwrap();
        assert!(root.exists("v2").unwrap());
        root.mv("v2", "v3").unwrap();
        assert!(!root.exists("v2").unwrap());
        assert!(root.exists("v3").unwrap());
        assert!(root.rm("v3").unwrap());
        assert!(!root.rm("v3").unwrap());

        assert!(root.export_to_json("v", None).unwrap().contains("age"));
        assert!(root.export_to_yaml("v").unwrap().contains("age"));

        w.commit().unwrap();
    }

    // The read view forwards the same relative paths.
    let r = table.read().unwrap();
    let root = r.rooted("data").unwrap();

    assert_eq!(root.load::<i64>("n").unwrap(), Some(6));
    assert_eq!(root.load_value("v").unwrap(), Some(user(30)));
    assert_eq!(root.kind("v").unwrap(), Some(EntryKind::File));
    assert!(root.get("v", "age").unwrap().is_some());

    {
        let leaf: Leaf<'static, i64> = root.fetch("n").unwrap().unwrap();
        assert_eq!(leaf.get().unwrap(), 6);
    }

    assert!(root.export_to_json("v", None).unwrap().contains("age"));
    assert!(root.export_to_yaml("v").unwrap().contains("age"));
}

#[cfg(feature = "serde")]
#[test]
fn rooted_views_forward_serde_values() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        let root = w.rooted("data").unwrap();
        root.store_serde_value("s", &7i64).unwrap();
        assert_eq!(root.load_serde_value::<i64>("s").unwrap(), Some(7));
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    let root = r.rooted("data").unwrap();
    assert_eq!(root.load_serde_value::<i64>("s").unwrap(), Some(7));
}

#[cfg(feature = "derive")]
mod scoped {
    use arbordb::{AData, ArborDb};

    #[derive(AData, Debug, Clone, PartialEq)]
    #[arbor(index(name = "by_age", columns(age)))]
    struct Profile {
        age: i64,
    }

    #[test]
    fn rooted_query_keeps_only_entities_under_the_root() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        // Each person owns one `profile` entity.
        table.create_indexes::<Profile>("*/profile").unwrap();

        {
            let w = table.write().unwrap();
            w.store::<Profile>(
                "alice/profile",
                &Profile {
                    age: 30
                },
            )
            .unwrap();
            w.store::<Profile>(
                "bob/profile",
                &Profile {
                    age: 40
                },
            )
            .unwrap();
            w.commit().unwrap();
        }

        let r = table.read().unwrap();

        // The table-global query sees both profiles.
        assert_eq!(r.find::<Profile>("by_age", &[]).unwrap().len(), 2);

        // A view rooted at `alice` keeps only the profile under it.
        let alice = r.rooted("alice").unwrap();
        assert_eq!(
            alice.find::<Profile>("by_age", &[]).unwrap(),
            vec![Profile {
                age: 30
            }]
        );
    }

    #[test]
    fn rooted_write_view_queries_under_its_root() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        table.create_indexes::<Profile>("*/profile").unwrap();

        {
            let w = table.write().unwrap();
            w.store::<Profile>(
                "alice/profile",
                &Profile {
                    age: 30
                },
            )
            .unwrap();
            w.store::<Profile>(
                "bob/profile",
                &Profile {
                    age: 40
                },
            )
            .unwrap();

            // A write view rooted at `alice` sees only the profile under it, reading the
            // transaction's own uncommitted index state.
            let alice = w.rooted("alice").unwrap();
            assert_eq!(
                alice.find::<Profile>("by_age", &[]).unwrap(),
                vec![Profile {
                    age: 30
                }]
            );
            assert_eq!(alice.query("by_age").run::<Profile>().unwrap().len(), 1);

            w.commit().unwrap();
        }
    }
}
