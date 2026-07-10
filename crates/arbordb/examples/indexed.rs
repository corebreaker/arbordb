//! Secondary indexes: declare one on a type, create + back-fill in one call, and
//! query it — empty-prefix (all, in index order), a prefix on the leading column,
//! and an exact match on the full column tuple.
//!
//! Run with: `cargo run --example indexed --features derive`

use arbordb::data::Scalar;
use arbordb::{AData, AdbResult, ArborDb};

#[derive(AData, Debug, Clone, PartialEq)]
#[arbor(index(name = "by_city_age", columns(city, age)))]
struct User {
    name: String,
    city: String,
    age:  i64,
}

fn main() -> AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let users = db.open_table("users")?;

    // Create + back-fill every index the type declares, scoped to `users/*`.
    users.create_indexes::<User>("users/*")?;

    {
        let w = users.write()?;
        for (id, name, city, age) in [
            ("1", "Alice", "paris", 30),
            ("2", "Bob", "lyon", 25),
            ("3", "Carol", "paris", 40),
        ] {
            let user = User {
                name: String::from(name),
                city: String::from(city),
                age,
            };

            w.store::<User>(&format!("users/{id}"), &user)?;
        }
        w.commit()?;
    }

    let r = users.read()?;

    // Everyone, in index order — by city, then age (an empty prefix matches all).
    let all: Vec<(String, i64)> = r
        .find::<User>("by_city_age", &[])?
        .into_iter()
        .map(|u| (u.city, u.age))
        .collect();
    println!("all: {all:?}");

    // Residents of Paris, ordered by age (a one-column prefix on the composite).
    let paris: Vec<(String, i64)> = r
        .find::<User>("by_city_age", &[Scalar::Str(String::from("paris"))])?
        .into_iter()
        .map(|u| (u.name, u.age))
        .collect();
    println!("paris: {paris:?}");

    // An exact match on the full column tuple.
    let exact = r.find::<User>("by_city_age", &[Scalar::Str(String::from("lyon")), Scalar::I64(25)])?;
    println!("lyon@25: {:?}", exact.iter().map(|u| &u.name).collect::<Vec<_>>());

    Ok(())
}
