//! Shared fixtures for the ArborDb benchmark suite.
//!
//! Every bench target (`reads`, `writes`, `indexes`) pulls its dataset, entity
//! type and helpers from here, so the numbers across categories describe the same
//! shape of data. Built behind the `derive` feature (the benches use a
//! `#[derive(AData)]` entity), so they run with `cargo bench --features derive`.
//!
//! Each target uses a subset of these helpers, hence the crate-wide `dead_code`
//! allowance.

#![allow(dead_code)]

use arbordb::{AData, ArborDb, Table};

/// Entities in a read-oriented fixture — enough for a realistic B-tree, small
/// enough that building it once per bench group (a single transaction) stays fast.
pub const DATASET: usize = 2_000;

/// Distinct keys cycled by the mutating benches, so the table never grows without
/// limit across criterion's many iterations.
pub const RING: usize = 500;

/// A flat record with an indexable column plus a few scalars of different types.
#[derive(AData, Clone)]
#[arbor(index(name = "by_age", columns(age)))]
pub struct User {
    pub name:   String,
    pub age:    u32,
    pub email:  String,
    pub score:  i64,
    pub active: bool,
}

impl User {
    /// A deterministic sample; the age is bucketed into 100 values so a `by_age`
    /// lookup returns many hits.
    pub fn sample(i: usize) -> User {
        User {
            name:   format!("user-{i}"),
            age:    (i % 100) as u32,
            email:  format!("user{i}@example.io"),
            score:  i as i64,
            active: i.is_multiple_of(2),
        }
    }
}

/// An in-memory database holding `count` users at `users/{i}`, written in a single
/// transaction. When `indexed`, the `by_age` index is created first, so every write
/// also pays index maintenance.
pub fn populated(count: usize, indexed: bool) -> (ArborDb, Table) {
    let db = ArborDb::create_in_memory().expect("create in-memory db");
    let table = db.open_table("users").expect("open table");

    if indexed {
        table.create_indexes::<User>("users/*").expect("create indexes");
    }

    let w = table.write().expect("begin write");
    for i in 0..count {
        w.store::<User>(format!("users/{i}"), &User::sample(i))
            .expect("store user");
    }
    w.commit().expect("commit");

    (db, table)
}

/// A fixture for the mutating benches: a [`RING`]-sized table plus the pre-built
/// paths and entities a bench cycles through, so per-iteration work measures the
/// database operation rather than `format!` / `String` allocation.
pub struct Ring {
    pub db:    ArborDb,
    pub table: Table,
    pub paths: Vec<String>,
    pub users: Vec<User>,
}

impl Ring {
    pub fn new(indexed: bool) -> Ring {
        let (db, table) = populated(RING, indexed);
        let paths = (0..RING).map(|i| format!("users/{i}")).collect();
        let users = (0..RING).map(User::sample).collect();

        Ring {
            db,
            table,
            paths,
            users,
        }
    }
}
