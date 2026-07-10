//! Filesystem-style storage without the derive macro: a dynamic [`Value`], plus
//! `ls` / `mv` / `cp`.
//!
//! Run with: `cargo run --example basic`

use std::collections::BTreeMap;

use arbordb::{data::Scalar, AdbResult, ArborDb, Value};

fn user(name: &str, age: i64) -> Value {
    Value::Node(BTreeMap::from([
        (String::from("name"), Value::Leaf(Scalar::Str(name.into()))),
        (String::from("age"), Value::Leaf(Scalar::I64(age))),
    ]))
}

fn main() -> AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let table = db.open_table("people")?;

    {
        let w = table.write()?;
        w.store_value("users/alice", &user("Alice", 30))?;
        w.store_value("users/bob", &user("Bob", 40))?;
        w.mv("users/bob", "users/robert")?; // relink: identity preserved, O(1)
        w.cp("users/alice", "archive/alice")?; // deep copy under fresh keys
        w.commit()?;
    }

    let r = table.read()?;

    println!("alice's age: {:?}", r.get_as::<i64>("users/alice", "age")?);

    let names: Vec<String> = r.ls("users")?.into_iter().map(|(name, _)| name).collect();
    println!("users/: {names:?}");

    println!("archived copy age: {:?}", r.get_as::<i64>("archive/alice", "age")?);

    Ok(())
}
