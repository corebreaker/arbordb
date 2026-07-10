//! The secondary-index public API: `create` / `ensure` / `delete` and back-fill.

#![cfg(feature = "derive")]

use arbordb::index::{IndexColumn, IndexDef};
use arbordb::path::VPath;
use arbordb::{AData, AdbError, ArborDb};

#[derive(AData)]
#[arbor(index(name = "by_age", columns(age)))]
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

fn by_age(name: &str, unique: bool) -> IndexDef {
    IndexDef::new(
        String::from(name),
        String::from("members/*"),
        vec![IndexColumn::asc(VPath::root().child_name("age"))],
        unique,
    )
}

#[test]
fn create_ensure_and_delete_declared_indexes() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    // The type declares one index; create + back-fill it.
    table.create_indexes::<Member>("members/*").unwrap();

    assert!(table.has_index("by_age").unwrap());
    assert_eq!(table.index_def("by_age").unwrap().unwrap(), by_age("by_age", false),);

    // Idempotent for the identical definition.
    table.create_indexes::<Member>("members/*").unwrap();
    assert!(table.has_index("by_age").unwrap());

    // Dropping the declared indexes removes exactly the one that existed.
    assert_eq!(table.delete_indexes::<Member>().unwrap(), 1);
    assert!(!table.has_index("by_age").unwrap());
    assert!(table.index_def("by_age").unwrap().is_none());

    // A second drop is a no-op.
    assert_eq!(table.delete_indexes::<Member>().unwrap(), 0);
}

#[test]
fn ensure_index_is_idempotent_by_name_while_create_index_rejects_a_clash() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let asc = by_age("i", false);
    let desc = IndexDef::new(
        String::from("i"),
        String::from("members/*"),
        vec![IndexColumn::desc(VPath::root().child_name("age"))],
        false,
    );

    table.ensure_index(&asc).unwrap();
    // `ensure_index` leaves the present index untouched, even with a divergent def.
    table.ensure_index(&desc).unwrap();
    assert_eq!(table.index_def("i").unwrap().unwrap(), asc);

    // `create_index` instead rejects a name clash with a different definition.
    assert!(matches!(table.create_index(&desc), Err(AdbError::SchemaMismatch(_))));
}

#[test]
fn create_index_backfills_pre_existing_data() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    // Write data BEFORE the index exists.
    {
        let w = table.write().unwrap();
        w.store::<Member>("members/a", &member("a", 30)).unwrap();
        w.store::<Member>("members/b", &member("b", 40)).unwrap();
        w.commit().unwrap();
    }

    // A unique index over the (distinct) pre-existing ages back-fills cleanly.
    table.create_index(&by_age("u_age", true)).unwrap();
    assert!(table.has_index("u_age").unwrap());
}

#[test]
fn a_unique_index_over_duplicate_data_fails_to_back_fill() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Member>("members/a", &member("a", 30)).unwrap();
        w.store::<Member>("members/b", &member("b", 30)).unwrap();
        w.commit().unwrap();
    }

    // The back-fill hits the duplicate age and rejects the unique index; because it
    // ran at all, the index's entries were actually built.
    assert!(matches!(
        table.create_index(&by_age("u_age", true)),
        Err(AdbError::UniqueViolation { .. }),
    ));

    // The failed creation left no index registered.
    assert!(!table.has_index("u_age").unwrap());
}
