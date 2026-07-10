//! `#[derive(AData)]` for structs: generated `AData` impl, accessors, and descriptor.

#![cfg(feature = "derive")]

use arbordb::{AData, ArborDb};

#[derive(AData, Debug, Clone, PartialEq)]
struct Point {
    x: i64,
    y: i64,
}

#[derive(AData, Debug, Clone, PartialEq)]
struct Person {
    name: String,
    age:  u32,
    home: Point,
}

#[test]
fn derived_struct_round_trips() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let alice = Person {
        name: String::from("Alice"),
        age:  30,
        home: Point {
            x: 1, y: 2
        },
    };

    {
        let w = table.write().unwrap();
        w.store::<Person>("people/alice", &alice).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Person>("people/alice").unwrap(), Some(alice));
    // A nested field is reachable by VPath into the one blob.
    assert_eq!(r.get_as::<i64>("people/alice", "home/x").unwrap(), Some(1));
    assert_eq!(r.load::<Person>("people/bob").unwrap(), None);
}

#[test]
fn derived_accessors_read_and_edit() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Person>(
            "p",
            &Person {
                name: String::from("Bob"),
                age:  40,
                home: Point {
                    x: 5, y: 6
                },
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    // Read accessor: infallible getters, lazy leaf reads, nested navigation.
    {
        let r = table.read().unwrap();
        let person: ArborPerson<'static> = r.fetch("p").unwrap().unwrap();
        assert_eq!(person.name().get().unwrap(), "Bob");
        assert_eq!(person.age().get().unwrap(), 40);
        assert_eq!(person.home().x().get().unwrap(), 5);
    }

    // Write accessor: edit a scalar and a nested scalar.
    {
        let w = table.write().unwrap();
        {
            let person: ArborPersonMut<'_> = w.fetch_mut("p").unwrap().unwrap();
            person.age_mut().set(&41).unwrap();
            person.home_mut().x_mut().set(&50).unwrap();
        }
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.get_as::<u32>("p", "age").unwrap(), Some(41));
    assert_eq!(r.get_as::<i64>("p", "home/x").unwrap(), Some(50));
}

#[test]
fn descriptor_lists_the_fields() {
    assert_eq!(ArborPersonDesc::TYPE_NAME, "Person");
    assert_eq!(ArborPersonDesc::FIELDS, &["name", "age", "home"]);
    assert_eq!(ArborPointDesc::FIELDS, &["x", "y"]);
}
