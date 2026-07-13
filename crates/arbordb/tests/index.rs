//! The secondary-index public API: `create` / `ensure` / `delete` and back-fill.

#![cfg(feature = "derive")]

use arbordb::{
    data::Scalar,
    index::{IndexColumn, IndexDef},
    path::VPath,
    AData,
    AdbError,
    ArborDb,
};

#[derive(AData, Debug, Clone, PartialEq)]
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
fn table_name_and_ensure_indexes_from_a_type() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("members").unwrap();

    assert_eq!(table.name(), "members");

    // `ensure_indexes` creates every index the type declares, idempotently by name.
    table.ensure_indexes::<Member>("members/*").unwrap();
    assert!(table.has_index("by_age").unwrap());

    table.ensure_indexes::<Member>("members/*").unwrap();
    assert!(table.has_index("by_age").unwrap());
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
fn dropping_an_index_with_entries_clears_its_key_block() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();
    table.create_indexes::<Member>("members/*").unwrap();

    // Store indexed data so the index holds physical entries to sweep.
    {
        let w = table.write().unwrap();
        w.store::<Member>("members/a", &member("a", 30)).unwrap();
        w.store::<Member>("members/b", &member("b", 40)).unwrap();
        w.commit().unwrap();
    }
    assert_eq!(table.read().unwrap().find::<Member>("by_age", &[]).unwrap().len(), 2);

    // Dropping the index scans its id block and removes every entry.
    assert_eq!(table.delete_indexes::<Member>().unwrap(), 1);
    assert!(!table.has_index("by_age").unwrap());
    assert!(matches!(
        table.read().unwrap().find::<Member>("by_age", &[]),
        Err(AdbError::IndexNotFound { .. })
    ));
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

#[test]
fn find_and_query_return_entities_in_index_order() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();
    table.create_indexes::<Member>("members/*").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Member>("members/c", &member("c", 30)).unwrap();
        w.store::<Member>("members/a", &member("a", 10)).unwrap();
        w.store::<Member>("members/b", &member("b", 20)).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Exact match on the age column.
    assert_eq!(
        r.find::<Member>("by_age", &[Scalar::I64(20)]).unwrap(),
        vec![member("b", 20)]
    );

    // Empty prefix: every indexed entity, ascending by age.
    assert_eq!(
        r.find::<Member>("by_age", &[]).unwrap(),
        vec![member("a", 10), member("b", 20), member("c", 30)],
    );

    // Reversed: descending by age.
    assert_eq!(
        r.query("by_age").reversed().run::<Member>().unwrap(),
        vec![member("c", 30), member("b", 20), member("a", 10)],
    );

    // An unknown index and an over-long prefix are reported.
    assert!(matches!(
        r.find::<Member>("nope", &[]),
        Err(AdbError::IndexNotFound { .. })
    ));
    assert!(matches!(
        r.find::<Member>("by_age", &[Scalar::I64(1), Scalar::I64(2)]),
        Err(AdbError::IndexArity { .. })
    ));
}

#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(index(name = "by_city_age", columns(city, age)))]
struct Resident {
    city: String,
    age:  i64,
}

fn resident(city: &str, age: i64) -> Resident {
    Resident {
        city: city.into(),
        age,
    }
}

#[test]
fn prefix_match_on_a_composite_index() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();
    table.create_indexes::<Resident>("residents/*").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Resident>("residents/1", &resident("paris", 40)).unwrap();
        w.store::<Resident>("residents/2", &resident("lyon", 25)).unwrap();
        w.store::<Resident>("residents/3", &resident("paris", 30)).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // A one-column prefix matches every resident of that city, ordered by age.
    assert_eq!(
        r.find::<Resident>("by_city_age", &[Scalar::Str(String::from("paris"))])
            .unwrap(),
        vec![resident("paris", 30), resident("paris", 40)],
    );

    // The full column tuple is an exact match.
    assert_eq!(
        r.find::<Resident>("by_city_age", &[Scalar::Str(String::from("lyon")), Scalar::I64(25)])
            .unwrap(),
        vec![resident("lyon", 25)],
    );
}
