//! Does a burst of edits to one value in a single transaction coalesce into one
//! re-encode and — the expensive part under `permissions` — one Ed25519 signature?
//!
//! Each timed rep opens a write transaction, edits one field of one value `M` times
//! through a mutable accessor, and commits. With the write-back cache, the value is
//! sealed once at commit, so the **incremental** cost of each extra edit is a cheap
//! in-place patch (well under a µs) rather than a fresh ~10µs signature. So the total
//! per transaction should climb only gently with `M` — if each edit re-signed, it would
//! climb by ~10µs per edit.
//!
//! Run with:
//! `cargo run -p arbordb --features "derive permissions" --release --example perm_multi_edit_probe`

use arbordb::{AData, ArborDb, Table};
use std::time::Instant;

#[derive(AData, Clone)]
struct Counter {
    score: i64,
    name:  String,
}

/// Edit counts swept per transaction.
const SIZES: [usize; 6] = [1, 2, 4, 8, 16, 32];

/// Median of this many timed reps per size.
const REPS: usize = 15;

/// One transaction that patches `x`'s `score` `edits` times, then commits.
fn burst(table: &Table, edits: usize, base: i64) {
    let w = table.write().expect("begin write");

    // Edit through the derived accessor's same-width scalar setter; the buffered value
    // is written (and, under `permissions`, sealed) once at commit.
    {
        let acc = w
            .fetch_mut::<ArborCounterMut>("x")
            .expect("fetch_mut")
            .expect("present");
        for i in 0..edits {
            acc.score_mut().set(&(base + i as i64)).expect("set");
        }
    }

    w.commit().expect("commit");
}

fn time_burst(table: &Table, edits: usize) -> f64 {
    // Warm up.
    for _ in 0..REPS {
        burst(table, edits, 0);
    }

    let mut samples = Vec::with_capacity(REPS);
    for r in 0..REPS {
        let start = Instant::now();
        burst(table, edits, (r * 1000) as i64);
        samples.push(start.elapsed().as_secs_f64() * 1e6);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());

    samples[REPS / 2]
}

fn run(label: &str, table: &Table) {
    // Seed the single value the bursts edit.
    {
        let w = table.write().expect("begin write");
        w.store::<Counter>(
            "x",
            &Counter {
                score: 0,
                name:  String::from("seed"),
            },
        )
        .expect("store");
        w.commit().expect("commit");
    }

    println!("\n{label}: M edits of one value in one transaction, median of {REPS}:\n");
    println!("      M     us/txn   incremental us/edit");

    let mut prev: Option<(usize, f64)> = None;
    for m in SIZES {
        let t = time_burst(table, m);
        let incr = match prev {
            Some((pm, pv)) => format!("{:>10.3}", (t - pv) / (m - pm) as f64),
            None => String::from("         -"),
        };
        prev = Some((m, t));

        println!("  {m:>5}   {t:>8.1}   {incr}");
    }
}

fn main() {
    let plain = {
        let db = ArborDb::create_in_memory().expect("create db");
        db.open_table("t").expect("open table")
    };
    let protected = {
        let db = ArborDb::create_in_memory().expect("create db");
        let db = db.change_password("bench-secret").expect("promote to master");
        db.open_table("t").expect("open table")
    };

    run("unprotected", &plain);
    run("protected", &protected);
}
