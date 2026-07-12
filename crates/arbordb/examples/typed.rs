//! Typed storage with `#[derive(AData)]`: whole-value load, a lazy read accessor
//! that navigates into the one blob, and a write accessor that edits one scalar.
//!
//! Run with: `cargo run --example typed --features derive`

use arbordb::{AData, AdbResult, ArborDb};

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

fn main() -> AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let table = db.open_table("people")?;

    let alice = Person {
        name: String::from("Alice"),
        age:  30,
        home: Point {
            x: 1, y: 2
        },
    };

    {
        let w = table.write()?;
        w.store::<Person>("alice", &alice)?;
        w.commit()?;
    }

    let r = table.read()?;

    // Whole-value load.
    println!("loaded: {:?}", r.load::<Person>("alice")?);

    // A read accessor navigates lazily into the one blob — no full decode.
    let view: ArborPerson<'static> = r.fetch("alice")?.expect("alice exists");
    println!("home.x via accessor: {}", view.home().x().get()?);

    // A write accessor edits one nested scalar (rewriting the blob). It borrows the
    // transaction, so drop it before committing.
    {
        let w = table.write()?;
        {
            let person: ArborPersonMut<'_> = w.fetch_mut("alice")?.expect("alice exists");
            person.age_mut().set(&31)?;
        }
        w.commit()?;
    }

    let r = table.read()?;
    println!("age after edit: {:?}", r.get_as::<u32>("alice", "age")?);

    Ok(())
}
