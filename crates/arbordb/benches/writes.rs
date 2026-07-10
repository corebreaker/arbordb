//! Write benchmarks. A whole value is re-encoded into one blob and linked into its
//! parent directory, so `store` measures that end-to-end cost — first without any
//! index, then with `by_age` maintained, isolating the per-write index overhead.
//!
//! The keys cycle through a bounded [`RING`] of pre-built paths and entities, so the
//! table never grows across criterion's iterations and the timing reflects the
//! database operation, not `format!` / allocation.
//!
//! Run with: `cargo bench -p arbordb --features derive --bench writes`.

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

mod common;
use common::{Ring, User, RING};

fn writes(c: &mut Criterion) {
    let plain = Ring::new(false);
    let indexed = Ring::new(true);

    let mut group = c.benchmark_group("writes");
    group.throughput(Throughput::Elements(1));

    // Store + commit one entity, overwriting an existing key — no index maintenance.
    let mut i = 0usize;
    group.bench_function("store_commit", |b| {
        b.iter(|| {
            let slot = i % RING;
            let w = plain.table.write().expect("begin write");
            w.store::<User>(black_box(&plain.paths[slot]), black_box(&plain.users[slot]))
                .expect("store");
            w.commit().expect("commit");
            i += 1;
        })
    });

    // The same write, now with the `by_age` index maintained on every commit.
    let mut j = 0usize;
    group.bench_function("store_commit_indexed", |b| {
        b.iter(|| {
            let slot = j % RING;
            let w = indexed.table.write().expect("begin write");
            w.store::<User>(black_box(&indexed.paths[slot]), black_box(&indexed.users[slot]))
                .expect("store");
            w.commit().expect("commit");
            j += 1;
        })
    });

    group.finish();
}

criterion_group!(benches, writes);
criterion_main!(benches);
