//! Integration tests for the `entry-timestamps` feature: per-vnode created /
//! modified / accessed datetimes held in the reserved `$inodes` table.

#![cfg(feature = "entry-timestamps")]

use arbordb::{data::Scalar, ArborDb, Value};
use std::{thread::sleep, time::Duration};

fn leaf(n: i64) -> Value {
    Value::Leaf(Scalar::I64(n))
}

#[test]
fn create_sets_all_three_times_together() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    let w = t.write().unwrap();
    w.store_value("a/b", &leaf(1)).unwrap();
    w.commit().unwrap();

    let times = t
        .read()
        .unwrap()
        .times("a/b")
        .unwrap()
        .expect("a stored file has times");

    // A freshly created vnode stamps all three from the same clock reading.
    assert_eq!(times.created(), times.modified());
    assert_eq!(times.created(), times.accessed());
}

#[test]
fn overwrite_bumps_modified_but_preserves_created_and_accessed() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    {
        let w = t.write().unwrap();
        w.store_value("a/b", &leaf(1)).unwrap();
        w.commit().unwrap();
    }
    let first = t.read().unwrap().times("a/b").unwrap().unwrap();

    // Advance the clock so the modified stamp is observably later.
    sleep(Duration::from_millis(5));

    {
        let w = t.write().unwrap();
        w.store_value("a/b", &leaf(2)).unwrap();
        w.commit().unwrap();
    }
    let second = t.read().unwrap().times("a/b").unwrap().unwrap();

    assert_eq!(
        second.created(),
        first.created(),
        "creation time is preserved across a rewrite"
    );

    assert!(
        second.modified() > first.modified(),
        "modification time advances on a rewrite"
    );

    assert_eq!(
        second.accessed(),
        first.accessed(),
        "a write does not bump the access time"
    );
}

#[test]
fn adding_a_child_bumps_the_parent_directory_modified_time() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    {
        let w = t.write().unwrap();
        w.store_value("dir/first", &leaf(1)).unwrap();
        w.commit().unwrap();
    }
    let before = t.read().unwrap().times("dir").unwrap().expect("a directory has times");

    sleep(Duration::from_millis(5));

    {
        let w = t.write().unwrap();
        w.store_value("dir/second", &leaf(2)).unwrap();
        w.commit().unwrap();
    }
    let after = t.read().unwrap().times("dir").unwrap().unwrap();

    assert_eq!(
        after.created(),
        before.created(),
        "the directory keeps its creation time"
    );

    assert!(
        after.modified() > before.modified(),
        "linking a new child bumps the directory"
    );
}

#[test]
fn mv_preserves_the_moved_node_times() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    {
        let w = t.write().unwrap();
        w.store_value("src/x", &leaf(1)).unwrap();
        w.commit().unwrap();
    }
    let before = t.read().unwrap().times("src/x").unwrap().unwrap();

    sleep(Duration::from_millis(5));

    {
        let w = t.write().unwrap();
        w.mv("src/x", "dst/y").unwrap();
        w.commit().unwrap();
    }
    let after = t.read().unwrap().times("dst/y").unwrap().unwrap();

    // A move relinks the same vnode — its identity and its timestamps are unchanged.
    assert_eq!(after.created(), before.created());
    assert_eq!(after.modified(), before.modified());
}

#[test]
fn times_is_none_for_an_absent_node() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    let w = t.write().unwrap();
    w.store_value("present", &leaf(1)).unwrap();
    w.commit().unwrap();

    assert!(t.read().unwrap().times("absent").unwrap().is_none());
}

#[test]
fn a_read_then_explicit_flush_advances_the_access_time() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    {
        let w = t.write().unwrap();
        w.store_value("a/b", &leaf(1)).unwrap();
        w.commit().unwrap();
    }
    // `times` is a metadata probe and does not itself count as an access.
    let created = t.read().unwrap().times("a/b").unwrap().unwrap();

    sleep(Duration::from_millis(5));

    {
        // A content read buffers a fresh access time; dropping the txn deposits it.
        let r = t.read().unwrap();
        let _ = r.load_value("a/b").unwrap();
    }
    db.flush_access_times().unwrap();

    let after = t.read().unwrap().times("a/b").unwrap().unwrap();
    assert_eq!(after.created(), created.created());
    assert_eq!(after.modified(), created.modified());
    assert!(
        after.accessed() > created.accessed(),
        "a read then flush advances the access time"
    );
}

#[test]
fn a_committed_write_flushes_buffered_access_times() {
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("t").unwrap();

    {
        let w = t.write().unwrap();
        w.store_value("x", &leaf(1)).unwrap();
        w.commit().unwrap();
    }
    let before = t.read().unwrap().times("x").unwrap().unwrap();

    sleep(Duration::from_millis(5));

    {
        let r = t.read().unwrap();
        let _ = r.load_value("x").unwrap();
    }

    // An unrelated committed write piggybacks the pending access-time flush.
    {
        let w = t.write().unwrap();
        w.store_value("y", &leaf(2)).unwrap();
        w.commit().unwrap();
    }

    let after = t.read().unwrap().times("x").unwrap().unwrap();
    assert!(
        after.accessed() > before.accessed(),
        "a later committed write flushes buffered access times"
    );
}
