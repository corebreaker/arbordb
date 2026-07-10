//! Read-only JSON / YAML export of a stored value, a rooted view, and an
//! in-memory [`Value`] subtree.
//!
//! Run with: `cargo run --example export`

use arbordb::{
    data::Scalar,
    export::{JsonExporter, YamlExporter},
    AdbResult,
    ArborDb,
    Value,
};

use std::collections::BTreeMap;

fn profile() -> Value {
    Value::Node(BTreeMap::from([
        (String::from("name"), Value::Leaf(Scalar::Str(String::from("Alice")))),
        (String::from("age"), Value::Leaf(Scalar::U32(30))),
        (String::from("active"), Value::Leaf(Scalar::Bool(true))),
        (String::from("avatar"), Value::Leaf(Scalar::Bytes(vec![0, 1, 2, 255]))),
        (
            String::from("tags"),
            Value::List(vec![
                Value::Leaf(Scalar::Str(String::from("admin"))),
                Value::Leaf(Scalar::Str(String::from("staff"))),
            ]),
        ),
    ]))
}

fn main() -> AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let table = db.open_table("people")?;

    {
        let w = table.write()?;
        w.store_value("users/alice", &profile())?;
        w.commit()?;
    }

    let r = table.read()?;

    // Export the value stored at an access path — compact then pretty JSON. Raw
    // bytes come out as Base64, and object fields in sorted order.
    println!("-- compact JSON --");
    println!("{}", r.export_to_json("users/alice", None)?);

    println!("\n-- pretty JSON (2-space indent) --");
    println!("{}", r.export_to_json("users/alice", Some(2))?);

    // The same value as YAML (block style, already newline-terminated).
    println!("\n-- YAML --");
    print!("{}", r.export_to_yaml("users/alice")?);

    // A rooted view resolves paths relative to its root, then exports the same way.
    let users = r.rooted("users")?;
    println!("\n-- rooted view: export \"alice\" --");
    println!("{}", users.export_to_json("alice", None)?);

    // An in-memory `Value` renders a subtree navigated by an intra-value path.
    let value = profile();
    println!("\n-- in-memory Value: subtree \"tags\" then leaf \"tags[0]\" --");
    println!("{}", value.export_to_json("tags", None)?);
    println!("{}", value.export_to_json("tags[0]", None)?);

    // A directory has no value of its own, so it cannot be exported.
    if let Err(err) = r.export_to_json("users", None) {
        println!("\nexporting a directory fails: {err}");
    }

    Ok(())
}
