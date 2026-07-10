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

#[derive(AData, Debug, Clone, PartialEq)]
enum Shape {
    Empty,
    Circle(f64),
    Rect(i64, i64),
    Named { w: i64, h: i64 },
}

#[test]
fn derived_enum_round_trips_all_variant_shapes() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let cases = [
        ("empty", Shape::Empty),
        ("circle", Shape::Circle(1.5)),
        ("rect", Shape::Rect(3, 4)),
        (
            "named",
            Shape::Named {
                w: 10, h: 20
            },
        ),
    ];

    {
        let w = table.write().unwrap();
        for (path, shape) in &cases {
            w.store::<Shape>(*path, shape).unwrap();
        }
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    for (path, shape) in &cases {
        assert_eq!(r.load::<Shape>(*path).unwrap(), Some(shape.clone()));
    }

    let rect: ArborShape<'static> = r.fetch("rect").unwrap().unwrap();
    assert_eq!(rect.variant().unwrap(), "Rect");

    assert_eq!(ArborShapeDesc::TYPE_NAME, "Shape");
    assert_eq!(ArborShapeDesc::VARIANTS, &["Empty", "Circle", "Rect", "Named"]);
}

#[test]
fn derived_enum_variant_replacement_is_clean() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Shape>("s", &Shape::Rect(1, 2)).unwrap();
        w.commit().unwrap();
    }
    {
        let w = table.write().unwrap();
        w.store::<Shape>("s", &Shape::Circle(9.0)).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Shape>("s").unwrap(), Some(Shape::Circle(9.0)));

    let shape: ArborShape<'static> = r.fetch("s").unwrap().unwrap();
    assert_eq!(shape.variant().unwrap(), "Circle");
}

#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(rename_all = "camelCase")]
struct Config {
    max_size:    u32,
    #[arbor(rename = "id")]
    identifier:  String,
    retry_count: u32,
}

#[test]
fn rename_and_rename_all_change_stored_names() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let cfg = Config {
        max_size:    64,
        identifier:  String::from("abc"),
        retry_count: 3,
    };

    {
        let w = table.write().unwrap();
        w.store::<Config>("cfg", &cfg).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Round-trips through the renamed nodes.
    assert_eq!(r.load::<Config>("cfg").unwrap(), Some(cfg));

    // Stored under the camelCase / renamed names...
    assert_eq!(r.get_as::<u32>("cfg", "maxSize").unwrap(), Some(64));
    assert_eq!(r.get_as::<u32>("cfg", "retryCount").unwrap(), Some(3));
    assert!(r.get("cfg", "id").unwrap().is_some());

    // ...never under the original identifiers.
    assert!(r.get("cfg", "max_size").unwrap().is_none());
    assert!(r.get("cfg", "identifier").unwrap().is_none());
    assert!(r.get("cfg", "retry_count").unwrap().is_none());

    // Desc lists the stored names; the accessor getter navigates to them.
    assert_eq!(ArborConfigDesc::FIELDS, &["maxSize", "id", "retryCount"]);

    let view: ArborConfig<'static> = r.fetch("cfg").unwrap().unwrap();
    assert_eq!(view.max_size().get().unwrap(), 64);
    assert_eq!(view.identifier().get().unwrap(), "abc");
}

// A "v1" writer whose camelCase field lands as "fullName" (an old name).
#[derive(AData)]
#[arbor(rename_all = "camelCase")]
struct RecordV1 {
    full_name: String,
    count:     u32,
}

// A "v2" reader: the stored name is "name", but the old "fullName" is accepted.
#[derive(AData, Debug, PartialEq)]
struct RecordV2 {
    #[arbor(rename = "name", alias = "fullName")]
    name:  String,
    count: u32,
}

// Writes BOTH "name" and "fullName", to prove the primary wins over the alias.
#[derive(AData)]
struct RecordBoth {
    name:  String,
    #[arbor(rename = "fullName")]
    old:   String,
    count: u32,
}

#[test]
fn field_alias_is_a_load_only_fallback() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<RecordV1>(
            "a",
            &RecordV1 {
                full_name: String::from("Zoe"),
                count:     3,
            },
        )
        .unwrap();
        w.store::<RecordBoth>(
            "b",
            &RecordBoth {
                name:  String::from("primary"),
                old:   String::from("legacy"),
                count: 7,
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // "name" absent, "fullName" present -> the alias fallback picks it up.
    assert_eq!(
        r.load::<RecordV2>("a").unwrap(),
        Some(RecordV2 {
            name:  String::from("Zoe"),
            count: 3,
        }),
    );

    // Both present -> the primary "name" wins over the alias "fullName".
    assert_eq!(
        r.load::<RecordV2>("b").unwrap(),
        Some(RecordV2 {
            name:  String::from("primary"),
            count: 7,
        }),
    );
}

#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(rename_all = "snake_case")]
enum Event {
    UserLoggedIn,
    #[arbor(rename = "click")]
    MouseClicked(i64, i64),
    #[arbor(alias = "removed")]
    Deleted,
}

// Writes an enum node tagged "removed" — an old name for `Event::Deleted`.
#[derive(AData)]
enum OldEvent {
    #[arbor(rename = "removed")]
    Gone,
}

#[test]
fn enum_rename_all_variant_rename_and_alias() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Event>("login", &Event::UserLoggedIn).unwrap();
        w.store::<Event>("click", &Event::MouseClicked(3, 4)).unwrap();
        w.store::<Event>("del", &Event::Deleted).unwrap();
        w.store::<OldEvent>("legacy", &OldEvent::Gone).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // rename_all snake_case, an explicit variant rename, and round-trips.
    assert_eq!(r.load::<Event>("login").unwrap(), Some(Event::UserLoggedIn));
    assert_eq!(r.load::<Event>("click").unwrap(), Some(Event::MouseClicked(3, 4)));
    assert_eq!(r.load::<Event>("del").unwrap(), Some(Event::Deleted));

    // The stored tags reflect the renames.
    let login: ArborEvent<'static> = r.fetch("login").unwrap().unwrap();
    assert_eq!(login.variant().unwrap(), "user_logged_in");

    let click: ArborEvent<'static> = r.fetch("click").unwrap().unwrap();
    assert_eq!(click.variant().unwrap(), "click");

    assert_eq!(ArborEventDesc::VARIANTS, &["user_logged_in", "click", "deleted"]);

    // A node tagged "removed" (an alias for Deleted) still loads.
    assert_eq!(r.load::<Event>("legacy").unwrap(), Some(Event::Deleted));
}
