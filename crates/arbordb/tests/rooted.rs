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
}
