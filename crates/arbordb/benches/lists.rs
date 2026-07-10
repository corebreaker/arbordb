//! List benchmarks: a list-bearing entity, exercising the operations whose cost
//! depends on the shape of an embedded `Vec` field.
//!
//! ArborDb never shreds a value — the whole `Doc`, list included, is one blob — so
//! this drops StratoDb's packed-vs-shredded contrast (there is no shredded regime to
//! compare against) and keeps the shapes that still differ:
//!
//! - `store` / `load` touch the whole entity (every element), re-encoding or recomposing one blob;
//! - `read_element` reads a single element straight from the blob, navigating to it by `VPath` without materializing
//!   the rest — ArborDb's headline zero-copy path, an O(1) offset-table jump independent of the list length;
//! - `update_element` overwrites one element through the typed mutable accessor; the value is one blob, so the write
//!   re-encodes it (the accepted O(blob) cost of a partial write, which is why ArborDb's win is on reads).
//!
//! Run with: `cargo bench -p arbordb --features derive --bench lists`.

use arbordb::{AData, ArborDb, Table};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

/// Elements in the list — a modest but realistic embedded collection. There is no
/// shredded store to build element-by-element here, so this is bounded only to keep
/// the gate's test-mode run fast.
const N: usize = 256;

/// The element the single-element benches target.
const MID: usize = N / 2;

/// A list-bearing entity: one `String` list plus a scalar, representative of a
/// document with an embedded collection.
#[derive(AData, Clone)]
struct Doc {
    title: String,
    items: Vec<String>,
}

fn sample() -> Doc {
    Doc {
        title: "a document with an embedded list".to_string(),
        items: (0..N).map(|i| format!("item-number-{i}")).collect(),
    }
}

/// The intra-value path of the element the single-element benches target.
fn mid_at() -> String {
    format!("items[{MID}]")
}

/// A table holding one `Doc` at `docs/0`.
fn populated() -> (ArborDb, Table) {
    let db = ArborDb::create_in_memory().expect("db");
    let table = db.open_table("docs").expect("table");

    let w = table.write().expect("write");
    w.store::<Doc>("docs/0", &sample()).expect("store");
    w.commit().expect("commit");

    (db, table)
}

fn lists(c: &mut Criterion) {
    let mut group = c.benchmark_group("lists");
    group.throughput(Throughput::Elements(1));

    // Store the whole entity, re-encoding one blob and committing each time.
    {
        let (_db, table) = populated();
        let doc = sample();

        group.bench_function("store", |b| {
            b.iter(|| {
                let w = table.write().expect("write");
                w.store::<Doc>("docs/0", black_box(&doc)).expect("store");
                w.commit().expect("commit");
            })
        });
    }

    // Load the whole entity — every element recomposed.
    {
        let (_db, table) = populated();

        group.bench_function("load", |b| {
            b.iter(|| {
                let r = table.read().expect("read");
                let d: Option<Doc> = r.load(black_box("docs/0")).expect("load");

                black_box(d);
            })
        });
    }

    // Read a single element — an O(1) offset-table jump straight out of the blob, no
    // sibling element decoded.
    {
        let (_db, table) = populated();
        let at = mid_at();

        group.bench_function("read_element", |b| {
            b.iter(|| {
                let r = table.read().expect("read");
                let v: Option<String> = r.get_as(black_box("docs/0"), black_box(&at)).expect("get");

                black_box(v);
            })
        });
    }

    // Update one element through the typed mutable accessor: one blob, so the write
    // re-encodes it — cost is O(blob), not O(1).
    {
        let (_db, table) = populated();
        let mut i = 0u64;

        group.bench_function("update_element", |b| {
            b.iter(|| {
                let w = table.write().expect("write");
                w.fetch_mut::<ArborDocMut>("docs/0")
                    .expect("fetch_mut")
                    .expect("present")
                    .items_mut()
                    .get(MID)
                    .expect("get")
                    .expect("in range")
                    .set(&format!("updated-{i}"))
                    .expect("set");
                w.commit().expect("commit");
                i += 1;
            })
        });
    }

    group.finish();
}

criterion_group!(benches, lists);
criterion_main!(benches);
