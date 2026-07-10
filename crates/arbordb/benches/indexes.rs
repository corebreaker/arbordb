//! Index-query benchmarks. The `by_age` index buckets ages into 100 values, so an
//! exact-age lookup returns roughly `DATASET / 100` hits, while an empty prefix
//! scans the whole index — each entity's full value recomposed from its blob.
//!
//! Run with: `cargo bench -p arbordb --features derive --bench indexes`.

use arbordb::data::Scalar;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

mod common;
use common::{User, DATASET};

fn indexes(c: &mut Criterion) {
    let (_db, table) = common::populated(DATASET, true);
    let r = table.read().expect("read txn");

    let mut group = c.benchmark_group("indexes");

    // A prefix lookup on the leading column, returning many hits (~DATASET / 100),
    // each recomposed into a full `User`.
    group.throughput(Throughput::Elements((DATASET / 100) as u64));
    group.bench_function("find_by_age", |b| {
        b.iter(|| {
            let key = [Scalar::U32(42)];
            let hits: Vec<User> = r.find(black_box("by_age"), black_box(&key)).expect("find");

            black_box(hits)
        })
    });

    // An empty prefix scans the whole index in order, recomposing every entity.
    group.throughput(Throughput::Elements(DATASET as u64));
    group.bench_function("scan_all", |b| {
        b.iter(|| {
            let hits: Vec<User> = r.find(black_box("by_age"), black_box(&[])).expect("find");

            black_box(hits)
        })
    });

    group.finish();
}

criterion_group!(benches, indexes);
criterion_main!(benches);
