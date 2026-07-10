//! Read benchmarks — ArborDb's headline path. A whole value is one blob, so the
//! contrast is between recomposing the entire entity and touching a single field:
//!
//! - `load_entity` — decode every field back into the typed struct;
//! - `get_one_field` — read one scalar leaf straight from the blob, zero-copy;
//! - `accessor_one_field` — the same single field through the typed read accessor.
//!
//! Run with: `cargo bench -p arbordb --features derive --bench reads`.

mod common;
use common::{ArborUser, User, DATASET};

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

fn reads(c: &mut Criterion) {
    let (_db, table) = common::populated(DATASET, false);
    let r = table.read().expect("read txn");

    let mid = DATASET / 2;
    let entity = format!("users/{mid}");
    let missing = format!("users/{}", DATASET + 1);

    let mut group = c.benchmark_group("reads");
    group.throughput(Throughput::Elements(1));

    // One scalar leaf read straight out of the blob — no sibling field is decoded.
    group.bench_function("get_one_field", |b| {
        b.iter(|| {
            let v: Option<u32> = r.get_as(black_box(&entity), black_box("age")).expect("get");

            black_box(v)
        })
    });

    // A presence test that resolves nothing (the absent-path fast exit).
    group.bench_function("kind_missing", |b| {
        b.iter(|| black_box(r.kind(black_box(&missing)).expect("kind")))
    });

    // Full entity recomposition: every field is resolved and decoded.
    group.bench_function("load_entity", |b| {
        b.iter(|| {
            let u: Option<User> = r.load(black_box(&entity)).expect("load");

            black_box(u)
        })
    });

    // One field through the typed read accessor: only that leaf is walked and
    // decoded — the rest of the blob is never visited.
    group.bench_function("accessor_one_field", |b| {
        b.iter(|| {
            let acc: ArborUser<'static> = r.fetch(black_box(&entity)).expect("fetch").expect("present");

            black_box(acc.age().get().expect("age"))
        })
    });

    group.finish();
}

criterion_group!(benches, reads);
criterion_main!(benches);
