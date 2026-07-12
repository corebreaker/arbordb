//! Scaling benchmarks — how the cost of touching *one* leaf tracks the *size* of the
//! value it lives in. This is where ArborDb's one-blob model shows both its strength
//! and its deliberate trade, so each measurement is swept over a growing value: an
//! object of N fields, or a list of N elements, each stored as a single blob.
//!
//! - `*_read_one_field` / `*_read_one_element` — read a single leaf straight from the blob, navigating the codec's
//!   offset tables (O(log N) to find an object field, O(1) to jump to a list element) without decoding any sibling.
//!   This is ArborDb's headline: the cost stays **flat** as the value grows.
//! - `*_load_all` — recompose the whole value. This is the linear-in-N baseline the partial read avoids; it is here
//!   only to make the flat read line legible against something that grows.
//! - `*_write_one_field` / `*_write_one_element` — change one leaf and persist it through `store_value`, which
//!   re-encodes and rewrites the whole blob: **O(N)**, growing with the value. This is the whole-value baseline; a
//!   same-width scalar edit through a `fetch_mut` accessor instead patches the blob in place (the in-place fast path,
//!   benched as `writes::set_field`), so it sidesteps this cost.
//!
//! It drives the dynamic `Value` API directly (no derived entity), so — unlike the
//! rest of the suite — it needs no `derive` feature.
//!
//! Run with: `cargo bench -p arbordb --bench scaling`.

use arbordb::{data::Scalar, ArborDb, Table, Value};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::{collections::BTreeMap, hint::black_box};

/// Field counts for the object sweep — up to a deliberately wide struct.
const OBJECT_SIZES: [usize; 4] = [8, 32, 128, 512];

/// Element counts for the list sweep — up to a long embedded collection.
const LIST_SIZES: [usize; 4] = [16, 256, 4096, 8192];

/// An object with `n` scalar fields named `f0000`..`f{n-1}` (zero-padded so the name
/// order matches the numeric order, and every field is a same-shaped leaf).
fn object_value(n: usize) -> Value {
    let mut map = BTreeMap::new();
    for i in 0..n {
        map.insert(format!("f{i:04}"), Value::new_leaf(Scalar::U64(i as u64)));
    }

    Value::new_node(map)
}

/// A list of `n` scalar elements.
fn list_value(n: usize) -> Value {
    Value::new_list((0..n).map(|i| Value::new_leaf(Scalar::U64(i as u64))).collect())
}

/// The name of the middle field of an `n`-field object.
fn mid_field(n: usize) -> String {
    format!("f{:04}", n / 2)
}

/// The intra-value path of the middle element of an `n`-element list.
fn mid_index(n: usize) -> String {
    format!("[{}]", n / 2)
}

/// An in-memory table holding `value` as the single file `x`.
fn stored(value: &Value) -> (ArborDb, Table) {
    let db = ArborDb::create_in_memory().expect("db");
    let table = db.open_table("t").expect("table");

    let w = table.write().expect("write");
    w.store_value("x", value).expect("store_value");
    w.commit().expect("commit");

    (db, table)
}

fn objects(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling/object");
    group.throughput(Throughput::Elements(1));

    for n in OBJECT_SIZES {
        // Read one field — flat as the object grows (no sibling field decoded).
        group.bench_with_input(BenchmarkId::new("read_one_field", n), &n, |b, &n| {
            let (_db, table) = stored(&object_value(n));
            let r = table.read().expect("read");
            let field = mid_field(n);

            b.iter(|| {
                let v: Option<u64> = r.get_as(black_box("x"), black_box(&field)).expect("get");

                black_box(v);
            })
        });

        // Recompose the whole object — the linear-in-N baseline the partial read avoids.
        group.bench_with_input(BenchmarkId::new("load_all", n), &n, |b, &n| {
            let (_db, table) = stored(&object_value(n));
            let r = table.read().expect("read");

            b.iter(|| black_box(r.load_value(black_box("x")).expect("load_value")))
        });

        // Change one field and persist — re-encodes the whole blob, so O(N).
        group.bench_with_input(BenchmarkId::new("write_one_field", n), &n, |b, &n| {
            let (_db, table) = stored(&object_value(n));
            let mut value = object_value(n);
            let field = mid_field(n);
            let mut ctr = 0u64;

            b.iter(|| {
                value.set_value(&field, Value::new_leaf(Scalar::U64(ctr)));
                ctr = ctr.wrapping_add(1);

                let w = table.write().expect("write");
                w.store_value("x", &value).expect("store_value");
                w.commit().expect("commit");
            })
        });
    }

    group.finish();
}

fn lists(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling/list");
    group.throughput(Throughput::Elements(1));

    for n in LIST_SIZES {
        // Read one element — an O(1) offset-table jump, flat as the list grows.
        group.bench_with_input(BenchmarkId::new("read_one_element", n), &n, |b, &n| {
            let (_db, table) = stored(&list_value(n));
            let r = table.read().expect("read");
            let at = mid_index(n);

            b.iter(|| {
                let v: Option<u64> = r.get_as(black_box("x"), black_box(&at)).expect("get");

                black_box(v);
            })
        });

        // Recompose the whole list — linear in N.
        group.bench_with_input(BenchmarkId::new("load_all", n), &n, |b, &n| {
            let (_db, table) = stored(&list_value(n));
            let r = table.read().expect("read");

            b.iter(|| black_box(r.load_value(black_box("x")).expect("load_value")))
        });

        // Change one element and persist — re-encodes the whole blob, so O(N).
        group.bench_with_input(BenchmarkId::new("write_one_element", n), &n, |b, &n| {
            let (_db, table) = stored(&list_value(n));
            let mut value = list_value(n);
            let at = mid_index(n);
            let mut ctr = 0u64;

            b.iter(|| {
                value.set_value(&at, Value::new_leaf(Scalar::U64(ctr)));
                ctr = ctr.wrapping_add(1);

                let w = table.write().expect("write");
                w.store_value("x", &value).expect("store_value");
                w.commit().expect("commit");
            })
        });
    }

    group.finish();
}

criterion_group!(benches, objects, lists);
criterion_main!(benches);
