//! `#[derive(AData)]` for structs: generated `AData` impl, accessors, and descriptor.

#![cfg(feature = "derive")]

use arbordb::{AData, ArborDb};
use std::marker::PhantomData;

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

// A plain type that does NOT implement AData — only Default. A `skip` field of
// this type must still compile (no accessor, no store, no load through AData).
#[derive(Debug, Default, PartialEq)]
struct Scratch {
    blob: Vec<u8>,
}

#[derive(AData, Debug, PartialEq)]
struct HasSkipped {
    id:      u32,
    #[arbor(skip)]
    scratch: Scratch,
}

#[test]
fn skip_excludes_a_field_from_storage_and_loads_the_default() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<HasSkipped>(
            "x",
            &HasSkipped {
                id:      3,
                scratch: Scratch {
                    blob: vec![1, 2, 3]
                },
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The skipped field is never stored, and loads as its Default.
    assert_eq!(
        r.load::<HasSkipped>("x").unwrap(),
        Some(HasSkipped {
            id:      3,
            scratch: Scratch::default(),
        }),
    );
    assert_eq!(r.get_as::<u32>("x", "id").unwrap(), Some(3));
    assert!(r.get("x", "scratch").unwrap().is_none());

    // Desc lists only the in-shape field.
    assert_eq!(ArborHasSkippedDesc::FIELDS, &["id"]);
}

fn fallback_tag() -> String {
    String::from("untagged")
}

#[derive(AData, Debug, PartialEq)]
struct WithSkipDefault {
    id:  u32,
    #[arbor(skip, default = "fallback_tag")]
    tag: String,
}

#[test]
fn skip_with_a_default_path_uses_that_function() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<WithSkipDefault>(
            "x",
            &WithSkipDefault {
                id:  1,
                tag: String::from("ignored-on-store"),
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(
        r.load::<WithSkipDefault>("x").unwrap(),
        Some(WithSkipDefault {
            id:  1,
            tag: String::from("untagged"),
        }),
    );
}

#[derive(AData, Debug, PartialEq)]
struct Timestamped {
    value:  i64,
    #[arbor(skip_load)]
    cached: u32,
}

#[test]
fn skip_load_stores_the_field_but_reads_the_default() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Timestamped>(
            "x",
            &Timestamped {
                value: 7, cached: 99
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The node IS written (99), but load ignores it and takes the default (0).
    assert_eq!(r.get_as::<u32>("x", "cached").unwrap(), Some(99));
    assert_eq!(
        r.load::<Timestamped>("x").unwrap(),
        Some(Timestamped {
            value: 7, cached: 0
        }),
    );
}

// A "v1" writer without the `retries` node.
#[derive(AData)]
struct SettingsV1 {
    name: String,
}

#[derive(AData, Debug, PartialEq)]
struct SettingsV2 {
    name:    String,
    #[arbor(default)]
    retries: u32,
}

#[test]
fn default_fills_an_absent_field() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<SettingsV1>(
            "old",
            &SettingsV1 {
                name: String::from("a"),
            },
        )
        .unwrap();
        w.store::<SettingsV2>(
            "new",
            &SettingsV2 {
                name:    String::from("b"),
                retries: 5,
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The v1 blob has no `retries` node -> the default (0) fills it.
    assert_eq!(
        r.load::<SettingsV2>("old").unwrap(),
        Some(SettingsV2 {
            name:    String::from("a"),
            retries: 0,
        }),
    );

    // A present value is read as usual.
    assert_eq!(
        r.load::<SettingsV2>("new").unwrap(),
        Some(SettingsV2 {
            name:    String::from("b"),
            retries: 5,
        }),
    );
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(AData, Debug, PartialEq)]
struct Sparse {
    id:    u32,
    #[arbor(skip_store_if = "is_zero", default)]
    count: u32,
}

#[test]
fn skip_store_if_conditionally_omits_a_field() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Sparse>(
            "zero",
            &Sparse {
                id: 1, count: 0
            },
        )
        .unwrap();
        w.store::<Sparse>(
            "some",
            &Sparse {
                id: 2, count: 7
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Predicate true -> the node is omitted; load falls back to the default.
    assert!(r.get("zero", "count").unwrap().is_none());
    assert_eq!(
        r.load::<Sparse>("zero").unwrap(),
        Some(Sparse {
            id: 1, count: 0
        }),
    );

    // Predicate false -> the node is written and read back.
    assert!(r.get("some", "count").unwrap().is_some());
    assert_eq!(
        r.load::<Sparse>("some").unwrap(),
        Some(Sparse {
            id: 2, count: 7
        }),
    );
}

// A module supplying both `store` and `load`: an f64 is stored as a rounded i64.
mod celsius {
    use arbordb::access::{Reader, Writer};
    use arbordb::data::AData;
    use arbordb::path::VPath;
    use arbordb::AdbResult;

    pub fn store<W: Writer>(value: &f64, writer: &W, at: &VPath) -> AdbResult<()> {
        AData::store(&(*value as i64), writer, at)
    }

    pub fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<f64> {
        Ok(<i64 as AData>::load(reader, at)? as f64)
    }
}

#[derive(AData, Debug, PartialEq)]
struct Reading {
    #[arbor(with = "celsius")]
    temp: f64,
}

#[test]
fn with_module_replaces_both_store_and_load() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Reading>(
            "x",
            &Reading {
                temp: 21.7
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The custom store rounded the f64 to an integer scalar...
    assert_eq!(r.get_as::<i64>("x", "temp").unwrap(), Some(21));

    // ...and the custom load reads it back as an f64.
    assert_eq!(
        r.load::<Reading>("x").unwrap(),
        Some(Reading {
            temp: 21.0
        })
    );
}

// Independent custom store/load functions: the stored value is doubled.
fn store_doubled<W: arbordb::access::Writer>(
    value: &i64,
    writer: &W,
    at: &arbordb::path::VPath,
) -> arbordb::AdbResult<()> {
    arbordb::data::AData::store(&(value * 2), writer, at)
}

fn load_halved<R: arbordb::access::Reader>(reader: &R, at: &arbordb::path::VPath) -> arbordb::AdbResult<i64> {
    Ok(<i64 as arbordb::data::AData>::load(reader, at)? / 2)
}

#[derive(AData, Debug, PartialEq)]
struct Doubled {
    #[arbor(store_with = "store_doubled", load_with = "load_halved")]
    n: i64,
}

#[test]
fn store_with_and_load_with_replace_each_side() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Doubled>(
            "x",
            &Doubled {
                n: 5
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Stored doubled on the way in...
    assert_eq!(r.get_as::<i64>("x", "n").unwrap(), Some(10));

    // ...and halved on the way out, round-tripping to the original.
    assert_eq!(
        r.load::<Doubled>("x").unwrap(),
        Some(Doubled {
            n: 5
        })
    );
}

// A delegated type: stored AS its inner u32 (no node of its own), rebuilt with From.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(from = "u32", into = "u32")]
struct Millis(u32);

impl From<u32> for Millis {
    fn from(n: u32) -> Self {
        Millis(n)
    }
}

impl From<Millis> for u32 {
    fn from(m: Millis) -> Self {
        m.0
    }
}

#[test]
fn from_into_delegates_the_whole_value() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Millis>("x", &Millis(1500)).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The on-disk form is a bare u32 — the delegated type has no node of its own.
    assert_eq!(r.load::<u32>("x").unwrap(), Some(1500));

    // ...and it rebuilds through From on load.
    assert_eq!(r.load::<Millis>("x").unwrap(), Some(Millis(1500)));
}

// A delegated type with a validating (fallible) conversion.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(try_from = "i64", into = "i64")]
struct Percent(i64);

impl TryFrom<i64> for Percent {
    type Error = String;

    fn try_from(n: i64) -> Result<Self, String> {
        if (0..=100).contains(&n) {
            Ok(Percent(n))
        } else {
            Err(format!("percent out of range: {n}"))
        }
    }
}

impl From<Percent> for i64 {
    fn from(p: Percent) -> Self {
        p.0
    }
}

#[test]
fn try_from_delegation_reports_conversion_failures() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Percent>("ok", &Percent(42)).unwrap();
        // A raw out-of-range i64 stored where a Percent is expected.
        w.store::<i64>("bad", &200).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // A valid stored value converts...
    assert_eq!(r.load::<Percent>("ok").unwrap(), Some(Percent(42)));

    // ...and an invalid one surfaces the TryFrom error as an AdbError.
    assert!(r.load::<Percent>("bad").is_err());
}

// Internal tagging: the tag lives in a named field alongside the flattened payload.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(tag = "type")]
enum Msg {
    Ping,
    Move { x: i64, y: i64 },
    Ids(i64, i64),
}

#[test]
fn internal_tagging_flattens_the_payload() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Msg>("ping", &Msg::Ping).unwrap();
        w.store::<Msg>(
            "move",
            &Msg::Move {
                x: 1, y: 2
            },
        )
        .unwrap();
        w.store::<Msg>("ids", &Msg::Ids(7, 8)).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Round-trips through every shape.
    assert_eq!(r.load::<Msg>("ping").unwrap(), Some(Msg::Ping));
    assert_eq!(
        r.load::<Msg>("move").unwrap(),
        Some(Msg::Move {
            x: 1, y: 2
        })
    );
    assert_eq!(r.load::<Msg>("ids").unwrap(), Some(Msg::Ids(7, 8)));

    // The tag sits in the `type` field, and the struct payload is flattened
    // (`x`/`y` directly under the node, not nested).
    let view: ArborMsg<'static> = r.fetch("move").unwrap().unwrap();
    assert_eq!(view.variant().unwrap(), "Move");
    assert_eq!(r.get_as::<i64>("move", "x").unwrap(), Some(1));
    assert_eq!(r.get_as::<i64>("move", "y").unwrap(), Some(2));

    // Tuple elements are flattened under decimal-string keys.
    assert_eq!(r.get_as::<i64>("ids", "0").unwrap(), Some(7));
    assert_eq!(r.get_as::<i64>("ids", "1").unwrap(), Some(8));
}

// Adjacent tagging: the tag and the payload live in two named fields.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(tag = "kind", content = "data")]
enum Payload {
    Empty,
    Num(i64),
    Pair { a: i64, b: i64 },
}

#[test]
fn adjacent_tagging_splits_tag_and_content() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Payload>("empty", &Payload::Empty).unwrap();
        w.store::<Payload>("num", &Payload::Num(5)).unwrap();
        w.store::<Payload>(
            "pair",
            &Payload::Pair {
                a: 1, b: 2
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Round-trips through every shape.
    assert_eq!(r.load::<Payload>("empty").unwrap(), Some(Payload::Empty));
    assert_eq!(r.load::<Payload>("num").unwrap(), Some(Payload::Num(5)));
    assert_eq!(
        r.load::<Payload>("pair").unwrap(),
        Some(Payload::Pair {
            a: 1, b: 2
        })
    );

    // The tag sits in `kind`; the payload sits in `data`.
    let view: ArborPayload<'static> = r.fetch("num").unwrap().unwrap();
    assert_eq!(view.variant().unwrap(), "Num");
    assert_eq!(r.get_as::<i64>("num", "data").unwrap(), Some(5));
    assert_eq!(r.get_as::<i64>("pair", "data/a").unwrap(), Some(1));
    assert_eq!(r.get_as::<i64>("pair", "data/b").unwrap(), Some(2));
}

// Untagged: the payload is stored bare; load tries each variant in order.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(untagged)]
enum Value {
    Int(i64),
    Text(String),
    Nothing,
}

#[test]
fn untagged_stores_bare_and_tries_variants_in_order() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Value>("int", &Value::Int(5)).unwrap();
        w.store::<Value>("text", &Value::Text(String::from("hi"))).unwrap();
        w.store::<Value>("nothing", &Value::Nothing).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The int is stored bare (loadable directly as an i64).
    assert_eq!(r.load::<i64>("int").unwrap(), Some(5));

    // Each round-trips by trying Int, then Text, then Nothing.
    assert_eq!(r.load::<Value>("int").unwrap(), Some(Value::Int(5)));
    assert_eq!(r.load::<Value>("text").unwrap(), Some(Value::Text(String::from("hi"))));
    assert_eq!(r.load::<Value>("nothing").unwrap(), Some(Value::Nothing));
}

// An external-tagged enum used to write an arbitrary unknown tag.
#[derive(AData)]
enum Foreign {
    #[arbor(rename = "Weird")]
    Weird,
}

// External tagging with a catch-all `other` variant.
#[derive(AData, Debug, Clone, PartialEq)]
enum Level {
    Low,
    High,
    #[arbor(other)]
    Unknown,
}

#[test]
fn other_variant_catches_unknown_tags() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Level>("low", &Level::Low).unwrap();
        w.store::<Level>("unknown", &Level::Unknown).unwrap();
        w.store::<Foreign>("weird", &Foreign::Weird).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // A known tag loads normally.
    assert_eq!(r.load::<Level>("low").unwrap(), Some(Level::Low));

    // The `other` variant's own tag, and any foreign tag, both land on it.
    assert_eq!(r.load::<Level>("unknown").unwrap(), Some(Level::Unknown));
    assert_eq!(r.load::<Level>("weird").unwrap(), Some(Level::Unknown));
}

// A tagged enum with a custom `expecting` message and no catch-all.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(expecting = "a traffic light color")]
enum Color {
    Red,
    Green,
}

#[test]
fn expecting_customizes_the_no_match_error() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Foreign>("weird", &Foreign::Weird).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    let error = r.load::<Color>("weird").unwrap_err();
    assert!(error.to_string().contains("a traffic light color"));
}

// A generic struct: the derive adds a `T: AData` bound and a PhantomData marker.
#[derive(AData, Debug, Clone, PartialEq)]
struct Wrapper<T> {
    label: String,
    value: T,
}

#[test]
fn generic_struct_round_trips() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let wrapped = Wrapper {
        label: String::from("answer"),
        value: 42_i64,
    };

    {
        let w = table.write().unwrap();
        w.store::<Wrapper<i64>>("x", &wrapped).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    assert_eq!(r.load::<Wrapper<i64>>("x").unwrap(), Some(wrapped));
    assert_eq!(r.get_as::<i64>("x", "value").unwrap(), Some(42));

    // The accessor is itself generic (`ArborWrapper<'t, T>`).
    let view: ArborWrapper<'static, i64> = r.fetch("x").unwrap().unwrap();
    assert_eq!(view.label().get().unwrap(), "answer");
    assert_eq!(view.value().get().unwrap(), 42);
}

// A generic enum with two type parameters.
#[derive(AData, Debug, Clone, PartialEq)]
enum Either<L, R> {
    Left(L),
    Right(R),
}

#[test]
fn generic_enum_round_trips() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Either<i64, String>>("l", &Either::Left(7)).unwrap();
        w.store::<Either<i64, String>>("r", &Either::Right(String::from("hi")))
            .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    assert_eq!(r.load::<Either<i64, String>>("l").unwrap(), Some(Either::Left(7)));
    assert_eq!(
        r.load::<Either<i64, String>>("r").unwrap(),
        Some(Either::Right(String::from("hi"))),
    );

    let view: ArborEither<'static, i64, String> = r.fetch("l").unwrap().unwrap();
    assert_eq!(view.variant().unwrap(), "Left");
}

// A type parameter used only in a skipped PhantomData field: not stored, and
// never routed through AData. Does NOT implement AData.
#[derive(Debug, Clone, PartialEq)]
struct Marker;

// `bound = ""` replaces the default `T: AData`, so `Phantomish<Marker>` is valid
// even though `Marker: AData` does not hold.
#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(bound = "")]
struct Phantomish<T> {
    value: i64,
    #[arbor(skip)]
    tag:   PhantomData<T>,
}

#[test]
fn bound_override_drops_the_default_adata_bound() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let value = Phantomish::<Marker> {
        value: 5,
        tag:   PhantomData,
    };

    {
        let w = table.write().unwrap();
        w.store::<Phantomish<Marker>>("x", &value).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<Phantomish<Marker>>("x").unwrap(), Some(value));
    assert_eq!(r.get_as::<i64>("x", "value").unwrap(), Some(5));
}

// A flattened inner struct: its fields merge into the parent's node.
#[derive(AData, Debug, Clone, PartialEq)]
struct Meta {
    created: i64,
    author:  String,
}

#[derive(AData, Debug, Clone, PartialEq)]
struct Doc {
    title: String,
    #[arbor(flatten)]
    meta:  Meta,
}

#[test]
fn flatten_merges_the_field_into_the_parent_node() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    let doc = Doc {
        title: String::from("T"),
        meta:  Meta {
            created: 100,
            author:  String::from("Ann"),
        },
    };

    {
        let w = table.write().unwrap();
        w.store::<Doc>("x", &doc).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // Round-trips.
    assert_eq!(r.load::<Doc>("x").unwrap(), Some(doc));

    // The inner fields sit directly on the parent node — there is no `meta` node.
    assert_eq!(r.get_as::<i64>("x", "created").unwrap(), Some(100));
    assert!(r.get("x", "author").unwrap().is_some());
    assert!(r.get("x", "meta").unwrap().is_none());

    // Desc lists only the named field; the flattened one has no node of its own.
    assert_eq!(ArborDocDesc::FIELDS, &["title"]);

    // The accessor for a flattened field opens on the parent node.
    let view: ArborDoc<'static> = r.fetch("x").unwrap().unwrap();
    assert_eq!(view.title().get().unwrap(), "T");
    assert_eq!(view.meta().created().get().unwrap(), 100);
    assert_eq!(view.meta().author().get().unwrap(), "Ann");
}

// `#[arbor(index(...))]` declares secondary indexes; the derive emits `AIndexed`.
#[derive(AData)]
#[arbor(index(name = "by_age", columns(age)))]
#[arbor(index(name = "by_city_age", columns(city, age desc), unique))]
struct Member {
    name: String,
    age:  u32,
    city: String,
}

#[test]
fn derive_declares_indexes() {
    use arbordb::index::{AIndexed, IndexColumn, IndexDef};
    use arbordb::path::VPath;

    let defs = Member::index_defs("members/*");

    assert_eq!(
        defs,
        vec![
            IndexDef::new(
                String::from("by_age"),
                String::from("members/*"),
                vec![IndexColumn::asc(VPath::root().child_name("age"))],
                false,
            ),
            IndexDef::new(
                String::from("by_city_age"),
                String::from("members/*"),
                vec![
                    IndexColumn::asc(VPath::root().child_name("city")),
                    IndexColumn::desc(VPath::root().child_name("age")),
                ],
                true,
            ),
        ],
    );
}

// A column names a renamed field: the index path uses the STORED name.
#[derive(AData)]
#[arbor(rename_all = "camelCase", index(name = "by_full", columns(full_name)))]
struct Account {
    full_name: String,
}

#[test]
fn index_column_uses_the_stored_field_name() {
    use arbordb::index::AIndexed;

    let defs = Account::index_defs("accounts/*");

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].columns()[0].path().to_string(), "fullName");
}

// A struct without any index declaration still implements `AIndexed`.
#[test]
fn a_struct_without_indexes_has_an_empty_index_set() {
    use arbordb::index::AIndexed;

    assert!(Member::index_defs("members/*").iter().all(|def| !def.name().is_empty()));
    assert_eq!(Point::index_defs("points/*"), vec![]);
}
