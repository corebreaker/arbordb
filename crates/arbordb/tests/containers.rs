//! Container `AData` adapters: `Vec`, `Option`, `BTreeMap`, and the `Bytes` newtype.

use arbordb::{
    data::{AIdentifiable, Bytes, Leaf, LeafMut, Map, MapMut, Opt, OptMut, Seq, SeqMut},
    entry::EntryKind,
    ArborDb,
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

#[test]
fn seq_accessor_reads_and_writes() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Vec<i64>>("nums", &vec![10, 20]).unwrap();
        w.commit().unwrap();
    }

    {
        let w = table.write().unwrap();
        {
            let seq: SeqMut<'_, i64> = w.fetch_mut("nums").unwrap().unwrap();
            assert_eq!(seq.len().unwrap(), 2);
            assert!(!seq.is_empty().unwrap());
            assert_eq!(seq.get(0).unwrap().unwrap().get().unwrap(), 10);
            assert!(seq.get(9).unwrap().is_none());

            seq.push(&30).unwrap();
            seq.get(1).unwrap().unwrap().set(&25).unwrap();
            let _ = seq.path();
        }

        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Vec<i64>>("nums").unwrap(), Some(vec![10, 25, 30]));

    let seq: Seq<'static, i64> = r.fetch("nums").unwrap().unwrap();
    assert!(!seq.is_empty().unwrap());
    let _ = seq.path();
    let first: Leaf<'static, i64> = seq.get(0).unwrap().unwrap();
    let _ = first.path();
}

#[test]
fn leaf_accessor_edge_cases() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Vec<i64>>("nums", &vec![1, 2]).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Loading an `Option` at an absent path yields `None`.
    assert_eq!(r.load::<Option<i64>>("missing").unwrap(), None);

    // A read leaf pointed at a list has no scalar of its own, so `get` errors — and
    // `path` still reports where it is anchored.
    let leaf: Leaf<'static, i64> = r.fetch("nums").unwrap().unwrap();
    assert!(leaf.get().is_err());
    let _ = leaf.path();

    {
        let w = table.write().unwrap();
        {
            let leaf: LeafMut<'_, i64> = w.fetch_mut("nums").unwrap().unwrap();
            assert!(leaf.get().is_err());
            let _ = leaf.path();
        }
        w.commit().unwrap();
    }
}

#[test]
fn descending_through_a_file_resolves_to_nothing() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<i64>("f", &1).unwrap();
        w.commit().unwrap();
    }

    // "f" is a file, so it has no children: resolving a path *through* it finds nothing.
    let r = table.read().unwrap();
    assert!(!r.exists("f/child").unwrap());
    assert_eq!(r.kind("f/child").unwrap(), None);
}

#[test]
fn navigating_a_value_path_into_the_wrong_kind_yields_nothing() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<i64>("leaf", &5).unwrap();
        w.store::<Vec<i64>>("list", &vec![1, 2]).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // A field name into a leaf, and an index into a leaf, both resolve to nothing.
    assert_eq!(r.get("leaf", "field").unwrap(), None);
    assert_eq!(r.get("leaf", "[0]").unwrap(), None);

    // A field name into a list resolves to nothing (a list has no named fields).
    assert_eq!(r.get("list", "field").unwrap(), None);
}

#[test]
fn pushing_a_container_through_a_mut_accessor_materializes_it() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Vec<Vec<i64>>>("m", &vec![vec![1, 2]]).unwrap();
        w.commit().unwrap();
    }

    {
        let w = table.write().unwrap();
        {
            // Pushing a `Vec` element drives the in-place writer to ensure a nested list.
            let seq: SeqMut<'_, Vec<i64>> = w.fetch_mut("m").unwrap().unwrap();
            seq.push(&vec![3, 4]).unwrap();
        }
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(
        r.load::<Vec<Vec<i64>>>("m").unwrap(),
        Some(vec![vec![1, 2], vec![3, 4]])
    );
}

#[test]
fn mut_accessors_over_the_wrong_kind_report_an_error() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<i64>("scalar", &5).unwrap();
        w.commit().unwrap();
    }

    {
        let w = table.write().unwrap();
        {
            // A list accessor over a scalar finds no list when asked its length.
            let seq: SeqMut<'_, i64> = w.fetch_mut("scalar").unwrap().unwrap();
            assert!(seq.len().is_err());

            // A map accessor over a scalar finds no object when asked its keys.
            let map: MapMut<'_, i64> = w.fetch_mut("scalar").unwrap().unwrap();
            assert!(map.keys().is_err());
        }
        w.commit().unwrap();
    }
}

#[test]
fn option_accessor_reads_and_writes() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Option<i64>>("some", &Some(7)).unwrap();
        w.store::<Option<i64>>("none", &None).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    let some: Opt<'static, i64> = r.fetch("some").unwrap().unwrap();
    assert!(some.is_some().unwrap());
    assert_eq!(some.get().unwrap().unwrap().get().unwrap(), 7);
    let _ = some.path();

    let none: Opt<'static, i64> = r.fetch("none").unwrap().unwrap();
    assert!(!none.is_some().unwrap());
    assert!(none.get().unwrap().is_none());

    {
        let w = table.write().unwrap();
        {
            let opt: OptMut<'_, i64> = w.fetch_mut("some").unwrap().unwrap();
            opt.set(Some(&9)).unwrap();
            let _ = opt.path();
        }
        w.commit().unwrap();
    }

    {
        let w = table.write().unwrap();
        {
            let opt: OptMut<'_, i64> = w.fetch_mut("some").unwrap().unwrap();
            opt.clear().unwrap();
        }
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Option<i64>>("some").unwrap(), Some(None));
}

#[test]
fn map_accessor_reads_and_writes() {
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
    let map: Map<'static, i64> = r.fetch("scores").unwrap().unwrap();
    assert_eq!(map.keys().unwrap(), vec![String::from("alice"), String::from("bob")]);
    assert_eq!(map.len().unwrap(), 2);
    assert!(!map.is_empty().unwrap());
    assert!(map.contains_key("alice").unwrap());
    assert!(!map.contains_key("carol").unwrap());
    assert_eq!(map.get("alice").unwrap().unwrap().get().unwrap(), 30);
    assert!(map.get("carol").unwrap().is_none());
    let _ = map.path();

    {
        let w = table.write().unwrap();
        {
            let map: MapMut<'_, i64> = w.fetch_mut("scores").unwrap().unwrap();
            assert_eq!(map.keys().unwrap().len(), 2);
            assert_eq!(map.get("bob").unwrap().unwrap().get().unwrap(), 40);
            assert!(map.get("carol").unwrap().is_none());

            map.insert("carol", &50).unwrap();
            assert!(map.remove("alice").unwrap());
            assert!(!map.remove("alice").unwrap());
            let _ = map.path();
        }

        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    let after = r.load::<BTreeMap<String, i64>>("scores").unwrap().unwrap();
    assert_eq!(after.get("carol"), Some(&50));
    assert!(!after.contains_key("alice"));
}
