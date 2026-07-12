//! Benchmarks for the dynamic-document path — the `Value` round-trip, with no typed
//! layer on top:
//!
//! - `load_value` recomposes the whole file blob into an owned `Value` tree;
//! - `get_value` / `set_value` are the in-memory, path-addressed accessors over a `Value` already in hand;
//! - `store_value` encodes a `Value` back into one blob and writes the file.
//!
//! Unlike StratoDb's bench of the same name, there is no `export` group: JSON/YAML
//! export is deferred in ArborDb, so there is nothing to measure yet.
//!
//! Run with: `cargo bench -p arbordb --features derive --bench dynamic_value`.

mod common;
use common::{DATASET, RING};

use arbordb::{data::Scalar, Value};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

fn dynamic_value(c: &mut Criterion) {
    let (_db, table) = common::populated(DATASET, false);
    let r = table.read().expect("read txn");
    let mid = DATASET / 2;
    let entity = format!("users/{mid}");

    let mut group = c.benchmark_group("dynamic_value");
    group.throughput(Throughput::Elements(1));

    // Blob → Value: a faithful, owned copy of the whole file value.
    group.bench_function("load_value", |b| {
        b.iter(|| {
            let v = r.load_value(black_box(&entity)).expect("load_value");

            black_box(v)
        })
    });

    // In-memory navigation: clone the subtree at a path out of a Value already held.
    {
        let value = r.load_value(&entity).expect("load_value").expect("present");

        group.bench_function("get_value", |b| {
            b.iter(|| black_box(value.get_value(black_box("name"))))
        });
    }

    // In-memory mutation: overwrite an existing leaf (bounded, no growth).
    {
        let mut value = r.load_value(&entity).expect("load_value").expect("present");
        let mut n = 0u32;

        group.bench_function("set_value", |b| {
            b.iter(|| {
                let ok = value.set_value("age", Value::Leaf(Scalar::U32(n)));
                n = n.wrapping_add(1);

                black_box(ok);
            })
        });
    }

    // Value → blob: encode and write the file, cycling a bounded ring of keys so the
    // table never grows across criterion's iterations.
    {
        let (_wdb, wtable) = common::populated(RING, false);
        let value = r.load_value(&entity).expect("load_value").expect("present");
        let mut i = 0usize;

        group.bench_function("store_value", |b| {
            b.iter(|| {
                let w = wtable.write().expect("begin write");
                w.store_value(format!("users/{}", i % RING), &value)
                    .expect("store_value");

                w.commit().expect("commit");
                i += 1;
            })
        });
    }

    group.finish();
}

criterion_group!(benches, dynamic_value);
criterion_main!(benches);
