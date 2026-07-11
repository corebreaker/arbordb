//! Is a fresh bulk load under one parent linear — and how much does protection cost
//! per item — on a *protected* database?
//!
//! Mirrors the comparison harness's `bulk_probe`, but with the `permissions` feature
//! on. Each timed rep stores `N` fresh files under one fresh parent directory in a
//! single transaction. Without the write-back directory cache under `permissions`,
//! every link re-encodes AND re-signs (Ed25519) the growing parent-directory blob, so
//! the per-item cost climbs with `N` (super-linear) and pays ~2 signatures per file;
//! with the cache the parent is encoded and signed once at commit, so the per-item
//! cost stays flat (`~x2` total per doubling of `N`) and pays ~1 signature per file.
//!
//! Run with:
//! `cargo run -p arbordb --features "derive permissions" --release --example perm_bulk_probe`

use arbordb::{AData, ArborDb, Table};
use std::time::Instant;

/// A small flat record — enough to exercise a real value blob per file.
#[derive(AData, Clone)]
struct User {
    name:  String,
    age:   u32,
    email: String,
    score: i64,
}

impl User {
    fn sample(i: usize) -> User {
        User {
            name:  format!("user-{i}"),
            age:   (i % 100) as u32,
            email: format!("user{i}@example.io"),
            score: i as i64,
        }
    }
}

/// The element counts swept, each a fresh bulk under its own parent.
const SIZES: [usize; 4] = [250, 500, 1000, 2000];

/// Median of this many timed reps per size.
const REPS: usize = 7;

/// One fresh bulk of `n` users under `parent`, in a single transaction.
fn bulk(table: &Table, parent: &str, users: &[User]) {
    let w = table.write().expect("begin write");
    for (i, user) in users.iter().enumerate() {
        w.store::<User>(format!("{parent}/{i}"), user).expect("store");
    }
    w.commit().expect("commit");
}

/// Times `bulk` `REPS` times under distinct parents, returning the median microseconds.
fn time_bulk(table: &Table, tag: &str, users: &[User], counter: &mut u64) -> f64 {
    // Warm up (also primes the redb file pages) under its own parent.
    bulk(table, &format!("{tag}-warm{}", *counter), users);
    *counter += 1;

    let mut samples = Vec::with_capacity(REPS);
    for _ in 0..REPS {
        let parent = format!("{tag}-{}", *counter);
        *counter += 1;

        let start = Instant::now();
        bulk(table, &parent, users);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());

    samples[REPS / 2]
}

fn main() {
    // One protected (master) database, and one plain database for reference. The
    // Argon2id key derivation behind `change_password` is paid once, here.
    let protected = {
        let db = ArborDb::create_in_memory().expect("create db");
        let db = db.change_password("bench-secret").expect("promote to master");
        db.open_table("t").expect("open table")
    };
    let plain = {
        let db = ArborDb::create_in_memory().expect("create db");
        db.open_table("t").expect("open table")
    };

    println!("Fresh bulk of N files under one parent (one transaction), median of {REPS}:\n");
    println!("      N   protected us    us/op    plain us    us/op   prot/plain");

    let (mut pc, mut qc) = (0u64, 0u64);
    let mut prev: Option<(usize, f64)> = None;
    for n in SIZES {
        let users: Vec<User> = (0..n).map(User::sample).collect();

        let prot = time_bulk(&protected, "p", &users, &mut pc);
        let plain_us = time_bulk(&plain, "q", &users, &mut qc);

        let doubling = match prev {
            Some((pn, pv)) if pn * 2 == n => format!("  (x{:.2} for 2x N)", prot / pv),
            _ => String::new(),
        };
        prev = Some((n, prot));

        println!(
            "  {n:>5}    {prot:>10.1}   {:>6.3}   {plain_us:>9.1}   {:>6.3}   {:>8.2}{doubling}",
            prot / n as f64,
            plain_us / n as f64,
            prot / plain_us,
        );
    }
}
