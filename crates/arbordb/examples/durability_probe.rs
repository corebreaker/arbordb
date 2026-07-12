//! A warm-loop probe: how much does relaxing durability speed up a burst of small
//! one-store-per-commit transactions? Contrasts the default [`Durability::Immediate`]
//! against [`Durability::None`] on a real file backend, where the difference — the
//! per-commit fsync — actually shows (an in-memory database never flushes, so the
//! two levels would look identical). Isolates the flush by overwriting one fixed
//! path, so each transaction does near-constant work and the commit dominates.

use arbordb::{data::Scalar, ArborDb, Durability, Value};
use std::{collections::BTreeMap, path::Path, time::Instant};

/// A tiny fixed-width record, so the overwrite patches in place and the per-txn
/// work stays near-constant (the commit is what we are measuring).
fn record(i: i64) -> Value {
    Value::Node(BTreeMap::from([(String::from("n"), Value::Leaf(Scalar::I64(i)))]))
}

/// Times `n` one-store-per-commit transactions at `durability`, in milliseconds.
fn run(path: &Path, durability: Durability, n: i64) -> f64 {
    let _ = std::fs::remove_file(path);
    let db = ArborDb::create(path).unwrap();
    let table = db.open_table("t").unwrap();

    let start = Instant::now();
    for i in 0..n {
        let w = table.write().unwrap().with_durability(durability);
        w.store_value("item", &record(i)).unwrap();
        w.commit().unwrap();
    }
    let ms = start.elapsed().as_secs_f64() * 1e3;

    drop(db);
    let _ = std::fs::remove_file(path);

    ms
}

fn main() {
    let n = 3_000;
    let dir = std::env::temp_dir();
    let immediate_path = dir.join("adb_durability_immediate.adb");
    let none_path = dir.join("adb_durability_none.adb");

    // Fresh database per level so neither warms the other.
    let immediate = run(&immediate_path, Durability::Immediate, n);
    let none = run(&none_path, Durability::None, n);

    println!("{n} one-store-per-commit transactions, file backend:");
    println!("  Immediate (default): {immediate:>8.1} ms");
    println!("  None (relaxed):      {none:>8.1} ms");
    println!(
        "  speedup:             {:>8.1}x",
        immediate / none.max(f64::MIN_POSITIVE)
    );
}
