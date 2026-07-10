# CLAUDE.md — ArborDb developer guide

## Language

All communication with the user is in **French**. Code, identifiers, comments, doc-strings, commit messages, and test names stay in **English**.

---

## Project overview

ArborDb is a typed, transactional, indexed document store written in Rust, layered over **redb** (kept fully opaque — no redb type ever surfaces in the public API). It is the successor to StratoDb (`W:\projects\development\rust\stratodb`, https://github.com/corebreaker/stratodb).

**The core idea.** Each stored value is **one `Value`** (a dynamic tree of objects, lists and scalar leaves) serialized into a **single blob in one redb entry**, navigated **zero-copy** by a dedicated codec — ArborDb's own "rkyv", written to exploit redb's copy-on-write B-trees and borrow the page bytes directly. This is the deliberate departure from StratoDb, which fully *shredded* every scalar into its own keyed node.

Repository: https://github.com/corebreaker/arbordb

---

## Workspace layout

```
arbordb/
├── Cargo.toml                  workspace root (resolver = 3, edition = 2024)
├── .rustfmt.toml               nightly-only fmt config (see Style section)
├── crates/
│   ├── arbordb/                runtime crate
│   │   ├── src/
│   │   ├── tests/
│   │   ├── examples/           runnable examples
│   │   ├── benches/            criterion benches (common/ fixtures + one file per feature area)
│   │   └── Cargo.toml
│   └── arbordb-derive/         proc-macro crate (#[derive(AData)]) — lands in phase 2
```

The workspace is `members = ["crates/*"]`. The out-of-tree benchmark-comparison tool (`support/`, currently git-ignored) is deferred to the very end of the project.

---

## Build and test — the full gate

Run these before every commit. All must be green:

```sh
cargo test --all-features --all-targets          # tests (lib + integration + benches in test mode)
cargo test --all-features --doc                  # doctests (--all-targets skips them)
cargo clippy --all-targets -- -D warnings        # lint — default features
cargo clippy --all-targets --features derive -- -D warnings   # lint — with derive (once it exists)
cargo +nightly fmt --check                        # formatter (nightly ONLY)
```

Big-number spot-check (once bignum lands): `--all-features` turns every `*-as-scalar` on, which compiles out the `*-as-data`-only path; exercise it with `cargo test -p arbordb --features bignum-as-data` and the matching clippy.

Convenience scripts (`cargo do <name>`, needs `cargo-run-script`; `gc` needs `cargo-sweep`): `fmt`, `gc`, `bench`, `bench-reports`.

---

## Code style

The style is **hand-formatted and airy**; `cargo +nightly fmt` is the only formatter (never stable `cargo fmt` — it would silently undo alignment and struct-literal expansion). Rules rustfmt alone does not enforce:

- **Field alignment:** in struct/enum definitions, struct-variant literals, and constructor calls with ≥ 2 named fields, pad names so types/values form a column (`path:     VPath`, `expected: &'static str`).
- **Statement groups:** separate logical groups with a blank line — after a guard/early-return, before a trailing `Ok(…)` or a final expression inside a long match arm, between a "header" block and a following loop.
- **Struct literals always expanded** (one field per line), even when short.
- **Import order:** `use crate::…` block first, then a blank line, then third-party/std. Order within a block is semantic, not alphabetical.
- **One concept per file.** Large modules become a directory + `mod.rs` that only declares sub-mods and re-exports.
- **Doc-comments:** `//!` on every file; `///` on every public item and every enum/struct field. Comments in code only when the *why* is non-obvious.
- **Error type:** `AdbError` / `AdbResult<T>` everywhere (never `anyhow`, never raw `Box<dyn Error>`).

---

## Commit messages

Each paragraph is **one physical line**; blank lines separate paragraphs. No column wrapping (no 72-char margin). Subject line is the first paragraph; optional body follows after a blank line.

---

## Architecture — locked decisions

**Storage model — one zero-copy blob per value.** A table maps a key to a serialized `Value`. There is no shredding, no per-scalar node, no inter-node tree walk. redb stays fully opaque.

**Keys & paths — three distinct notions:**
- `APath` — the access path *and* the key: a filesystem-like path of names only (no list index). It is encoded order-preservingly straight into the redb key, so `store(apath, v)` / `load(apath)` are direct point ops and listing a "directory" is a prefix scan. There is **no separate path→key index table** — the key is a pure function of the path.
- `AKey` — the type of that redb key (the encoded form of an `APath`). It identifies one whole stored value, never a `Scalar` or a node inside a value. The public API is `APath`-based; opaque value-ids inside data use `Uuid`, not `AKey`.
- `VPath` — an intra-value path (`Name` + `Index [i]` segments) that navigates WITHIN a `Value` to a `Scalar`. It is the successor to StratoDb's `SPath`.

**The codec (ArborDb's own "rkyv").** `Value` (`Node(BTreeMap)` / `List(Vec)` / `Leaf(Scalar)`) ⟷ a byte blob laid out with offset tables: a leaf is `Scalar::encode`; a list is a count + a u32 offset table (O(1) element jump); an object is a count + a name-sorted `(name, offset)` table (O(log n) field lookup). An `ArchivedValue<'a>` borrows the redb page bytes and reads scalars in place — no decode into an owned tree, no alignment requirement (all reads are explicit `from_be_bytes` over unaligned slices).

**Flows.** *Read:* APath → AKey → borrowed blob → `ArchivedValue` → an `ArborXxx` accessor whose `get()` methods walk the bytes zero-copy → `Scalar`/typed field. *Write:* `store(apath, &T)` drives `AData::store` into an in-memory `Value` builder, encodes it to one blob, one redb `put`. A partial write rewrites the blob (the zero-copy win is on reads).

**Typed model reused from StratoDb (rename `S→A`, `Strato→Arbor`).** `Scalar` (+ the bignum feature matrix), `Value`, `AValue` (was `SValue`), `AData` (was `SData`), the entire `arbordb-derive` macro with all `#[arbor(...)]` attributes, the containers (`Leaf`/`Seq`/`Map`/`Opt`/`Bytes`), the accessors (`ArborXxx`/`ArborXxxMut`/`ArborXxxDesc`), `ARef`/`AMut`/`AIdentifiable`/`AIndexed`, `NodeKind`. The `Reader`/`Writer` traits are reused, but their internal node locator changes from `Skey` to an **offset into the blob** (`NodeRef`) — since an `AKey` does not address a scalar. The accessor still exposes the value's `AKey` and its `VPath` base for identity.

**Index model (later phase).** Named, composite (ordered columns), per-column ASC/DESC, optional uniqueness, order-preserving key encoding, pattern scope, query builder, back-fill — as in StratoDb, but a scope is a pattern over APaths, a column is a `VPath` into each value, and an index entry's entity identity is the value's `AKey`.

**Deferred (do not implement until requested):** JSON/YAML export; the benchmark-comparison tool.

---

## Roadmap

1. **Foundations** — `AdbError`/`AdbResult`, `Scalar` + byte codec, `Value` tree, `AKey`, `APath`/`VPath`/`Segment`, the zero-copy codec (`Value` → blob + `ArchivedValue`), `ArborDb`/`Table`/`ReadTxn`/`WriteTxn`, dynamic `store_value`/`load_value` round-trip on redb, `get`/`put`/`remove`/`kind`.
2. **Typed data** — `AValue`/`AData`, containers, accessors, `fetch`/`fetch_mut`/`store`/`load`, `#[derive(AData)]` for structs and enums, `Desc`.
3. **Derive attribute parity** (7 phases) + the **bignum** feature matrix.
4. **Secondary indexes**.
5. **Docs & polish** — rooted views, README, rustdoc, examples, cross-feature tests, benches, CI (CircleCI + GitHub Actions).

Deferred to the end: JSON/YAML export, benchmark-comparison tool.

---

## Key invariants to preserve

- **redb stays opaque.** Never let a `redb::` type appear in the public API.
- **`cargo +nightly fmt` only.** The `.rustfmt.toml` uses nightly-only keys that stable fmt silently ignores.
- **`AKey` addresses a value, not a scalar.** Navigation inside a value is always by `VPath` over the `ArchivedValue`, never by key.
- **No flat path→key index.** The redb key IS the encoded `APath`, never a separate `APath→AKey` mapping. StratoDb dropped exactly such an index because list-index shifts made it O(N·M); ArborDb's APaths carry no index and lists are intra-value, so the problem cannot recur.
- **The zero-copy win is on reads.** `ArchivedValue` borrows the redb page; a whole-value write re-encodes one blob.
- **Edition 2024 let-chains** are used freely; do not downgrade to nested `match`/`if let`.
