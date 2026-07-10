//! Container `AData` adapters: `Vec`, `Option`, `BTreeMap`, and the `Bytes` newtype.

use arbordb::{
    data::{Bytes, Seq},
    ArborDb,
    EntryKind,
};
use std::collections::BTreeMap;

#[test]
fn vec_roundtrips_and_navigates() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Vec<i64>>("nums", &vec![10, 20, 30]).unwrap();
        w.store::<Vec<i64>>("empty", &Vec::new()).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Vec<i64>>("nums").unwrap(), Some(vec![10, 20, 30]));
    assert_eq!(r.load::<Vec<i64>>("empty").unwrap(), Some(Vec::new()));

    let seq: Seq<'static, i64> = r.fetch("nums").unwrap().unwrap();
    assert_eq!(seq.len().unwrap(), 3);
    assert_eq!(seq.get(1).unwrap().unwrap().get().unwrap(), 20);
    assert!(seq.get(9).unwrap().is_none());
}

#[test]
fn option_roundtrips_some_and_none() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Option<i64>>("some", &Some(7)).unwrap();
        w.store::<Option<i64>>("none", &None).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Option<i64>>("some").unwrap(), Some(Some(7)));
    assert_eq!(r.load::<Option<i64>>("none").unwrap(), Some(None));
}

#[test]
fn map_roundtrips() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let mut scores = BTreeMap::new();
    scores.insert(String::from("alice"), 30i64);
    scores.insert(String::from("bob"), 40i64);

    {
        let w = table.write().unwrap();
        w.store::<BTreeMap<String, i64>>("scores", &scores).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<BTreeMap<String, i64>>("scores").unwrap(), Some(scores));
}

#[test]
fn bytes_is_stored_as_one_leaf() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Bytes>("blob", &Bytes(vec![1, 2, 3, 255])).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Bytes>("blob").unwrap(), Some(Bytes(vec![1, 2, 3, 255])));
    assert_eq!(r.kind("blob").unwrap(), Some(EntryKind::File));
}

#[test]
fn nested_containers_roundtrip() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let data: Vec<Option<i64>> = vec![Some(1), None, Some(3)];

    {
        let w = table.write().unwrap();
        w.store::<Vec<Option<i64>>>("xs", &data).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Vec<Option<i64>>>("xs").unwrap(), Some(data));
}
