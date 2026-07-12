//! Write benchmarks. `store_commit` re-encodes a whole value into one blob and links
//! it into its parent directory — first without any index, then with `by_age`
//! maintained, isolating the per-write index overhead. `set_field` contrasts that
//! with a single-scalar edit through the typed accessor: overwriting a same-width
//! leaf patches the blob in place, without decoding or re-encoding the whole value.
//!
//! The keys cycle through a bounded [`RING`] of pre-built paths and entities, so the
//! table never grows across criterion's iterations and the timing reflects the
//! database operation, not `format!` / allocation.
//!
//! Run with: `cargo bench -p arbordb --features derive --bench writes`.

mod common;
use common::{ArborUserMut, Ring, User, RING};

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

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

    // Overwrite one fixed-width scalar field through the typed accessor. The new `i64`
    // keeps the leaf's byte width, so the blob is patched in place — no decode, no
    // re-encode — unlike `store_commit`, which rebuilds the whole value.
    let mut k = 0usize;
    group.bench_function("set_field", |b| {
        b.iter(|| {
            let slot = k % RING;
            let w = plain.table.write().expect("begin write");
            w.fetch_mut::<ArborUserMut>(black_box(&plain.paths[slot]))
                .expect("fetch_mut")
                .expect("present")
                .score_mut()
                .set(black_box(&(k as i64)))
                .expect("set");
            w.commit().expect("commit");
            k += 1;
        })
    });

    group.finish();
}

criterion_group!(benches, writes);
criterion_main!(benches);
