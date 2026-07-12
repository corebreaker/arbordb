//! Integration tests for `store_serde_value` / `load_serde_value` (the `serde`
//! feature): a Serde value stored natively round-trips through the public API, and
//! lands as real objects and leaves the normal read API can navigate.

#![cfg(feature = "serde")]

use arbordb::ArborDb;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Account {
    id:      u64,
    name:    String,
    active:  bool,
    balance: f64,
    tags:    Vec<String>,
    meta:    BTreeMap<String, i64>,
}

fn sample() -> Account {
    Account {
        id:      7,
        name:    String::from("alice"),
        active:  true,
        balance: 12.5,
        tags:    vec![String::from("vip"), String::from("beta")],
        meta:    BTreeMap::from([(String::from("visits"), 3), (String::from("logins"), 9)]),
    }
}

#[test]
fn store_and_load_serializable_round_trips() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("accounts").unwrap();

    {
        let w = t.write().unwrap();
        w.store_serde_value("acme/alice", &sample()).unwrap();
        w.commit().unwrap();
    }

    let r = t.read().unwrap();
    let back: Account = r.load_serde_value("acme/alice").unwrap().unwrap();
    assert_eq!(back, sample());

    // Stored natively, not as one opaque blob: individual fields are reachable
    // through the ordinary read API, including into a nested map and a list.
    assert_eq!(
        r.get_as::<String>("acme/alice", "name").unwrap(),
        Some(String::from("alice"))
    );
    assert_eq!(r.get_as::<bool>("acme/alice", "active").unwrap(), Some(true));
    assert_eq!(r.get_as::<i64>("acme/alice", "meta/visits").unwrap(), Some(3));
    assert_eq!(
        r.get_as::<String>("acme/alice", "tags[0]").unwrap(),
        Some(String::from("vip"))
    );

    // A missing path deserializes to `None`.
    assert!(r.load_serde_value::<Account>("acme/bob").unwrap().is_none());
}

#[test]
fn rooted_views_forward_serde() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("accounts").unwrap();

    {
        let w = t.write().unwrap();
        let rooted = w.rooted("acme").unwrap();
        rooted.store_serde_value("alice", &sample()).unwrap();
        w.commit().unwrap();
    }

    let r = t.read().unwrap();
    let rooted = r.rooted("acme").unwrap();
    let back: Account = rooted.load_serde_value("alice").unwrap().unwrap();
    assert_eq!(back, sample());
}
