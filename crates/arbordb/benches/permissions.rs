//! Overhead of the `permissions` feature: the same operations on a **protected**
//! database versus an **unprotected** one.
//!
//! A protected write seals a per-value keyed BLAKE3 MAC (over the value bytes and
//! its ACL); a protected read verifies that MAC, checks the ACL, and — because the
//! shared path cache is bypassed for protected databases — re-walks the directory
//! tree each time. Both arms are compiled with `entry-timestamps` (implied by
//! `permissions`), so each already writes and reads the inode side table; the delta
//! therefore isolates the authentication / ACL / MAC layer, not the timestamps.
//!
//! The protected arm acts as the master user (who bypasses ACLs but still pays MAC
//! verification and the cache bypass), so the numbers reflect the mandatory cost of
//! protection, not a denial.
//!
//! Run with: `cargo bench -p arbordb --features "derive permissions" --bench permissions`.

mod common;
use common::{ArborUserMut, User, RING};

use arbordb::{ArborDb, Table};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use std::hint::black_box;

/// A [`RING`]-sized in-memory fixture, either promoted to a protected (master)
/// database or left unprotected, populated in a single transaction.
struct Fixture {
    // Kept alive for the table's lifetime; the benches drive `table` directly.
    _db:   ArborDb,
    table: Table,
    paths: Vec<String>,
    users: Vec<User>,
}

impl Fixture {
    fn new(protected: bool) -> Fixture {
        let db = ArborDb::create_in_memory().expect("create in-memory db");
        let db = if protected {
            db.change_password("bench-secret").expect("promote to master")
        } else {
            db
        };

        let table = db.open_table("users").expect("open table");
        {
            let w = table.write().expect("begin write");
            for i in 0..RING {
                w.store::<User>(format!("users/{i}"), &User::sample(i))
                    .expect("store user");
            }
            w.commit().expect("commit");
        }

        Fixture {
            _db: db,
            table,
            paths: (0..RING).map(|i| format!("users/{i}")).collect(),
            users: (0..RING).map(User::sample).collect(),
        }
    }
}

fn permissions(c: &mut Criterion) {
    let plain = Fixture::new(false);
    let protected = Fixture::new(true);

    // Store + commit one entity, overwriting an existing key. The protected arm also
    // seals a fresh MAC over the re-encoded blob and its ACL.
    {
        let mut group = c.benchmark_group("perm_store_commit");
        group.throughput(Throughput::Elements(1));

        let mut i = 0usize;
        group.bench_function("unprotected", |b| {
            b.iter(|| {
                let slot = i % RING;
                let w = plain.table.write().expect("begin write");
                w.store::<User>(black_box(&plain.paths[slot]), black_box(&plain.users[slot]))
                    .expect("store");
                w.commit().expect("commit");
                i += 1;
            })
        });

        let mut j = 0usize;
        group.bench_function("protected", |b| {
            b.iter(|| {
                let slot = j % RING;
                let w = protected.table.write().expect("begin write");
                w.store::<User>(black_box(&protected.paths[slot]), black_box(&protected.users[slot]))
                    .expect("store");
                w.commit().expect("commit");
                j += 1;
            })
        });

        group.finish();
    }

    // Overwrite one same-width scalar through the typed accessor (an in-place patch).
    // The protected arm verifies the current blob before patching, then re-seals.
    {
        let mut group = c.benchmark_group("perm_set_field");
        group.throughput(Throughput::Elements(1));

        let mut i = 0usize;
        group.bench_function("unprotected", |b| {
            b.iter(|| {
                let slot = i % RING;
                let w = plain.table.write().expect("begin write");
                w.fetch_mut::<ArborUserMut>(black_box(&plain.paths[slot]))
                    .expect("fetch_mut")
                    .expect("present")
                    .score_mut()
                    .set(black_box(&(i as i64)))
                    .expect("set");
                w.commit().expect("commit");
                i += 1;
            })
        });

        let mut j = 0usize;
        group.bench_function("protected", |b| {
            b.iter(|| {
                let slot = j % RING;
                let w = protected.table.write().expect("begin write");
                w.fetch_mut::<ArborUserMut>(black_box(&protected.paths[slot]))
                    .expect("fetch_mut")
                    .expect("present")
                    .score_mut()
                    .set(black_box(&(j as i64)))
                    .expect("set");
                w.commit().expect("commit");
                j += 1;
            })
        });

        group.finish();
    }

    // Recompose a whole entity. The protected arm walks the path with ACL + MAC
    // checks (no path cache) and verifies the value blob before decoding.
    {
        let mut group = c.benchmark_group("perm_load_entity");
        group.throughput(Throughput::Elements(1));
        let slot = RING / 2;

        let pr = plain.table.read().expect("read txn");
        group.bench_function("unprotected", |b| {
            b.iter(|| {
                let u: Option<User> = pr.load(black_box(&plain.paths[slot])).expect("load");

                black_box(u)
            })
        });

        let sr = protected.table.read().expect("read txn");
        group.bench_function("protected", |b| {
            b.iter(|| {
                let u: Option<User> = sr.load(black_box(&protected.paths[slot])).expect("load");

                black_box(u)
            })
        });

        group.finish();
    }

    // Read one scalar leaf. The protected arm still pays the full path walk and MAC
    // verification even though only one field is returned.
    {
        let mut group = c.benchmark_group("perm_get_one_field");
        group.throughput(Throughput::Elements(1));
        let slot = RING / 2;

        let pr = plain.table.read().expect("read txn");
        group.bench_function("unprotected", |b| {
            b.iter(|| {
                let v: Option<u32> = pr.get_as(black_box(&plain.paths[slot]), black_box("age")).expect("get");

                black_box(v)
            })
        });

        let sr = protected.table.read().expect("read txn");
        group.bench_function("protected", |b| {
            b.iter(|| {
                let v: Option<u32> = sr
                    .get_as(black_box(&protected.paths[slot]), black_box("age"))
                    .expect("get");

                black_box(v)
            })
        });

        group.finish();
    }
}

criterion_group!(benches, permissions);
criterion_main!(benches);
