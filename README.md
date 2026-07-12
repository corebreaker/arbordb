# ArborDb

A typed, transactional, indexed document store for Rust, layered over an embedded key-value engine.

ArborDb stores each structured value — a tree of objects, 
lists and scalar leaves — as **one zero-copy blob** in a single engine entry,
arranged as a **virtual filesystem** of directories and files. A field is read straight out of the engine's page bytes,
without decoding the rest of the value. On top of that sit a derive macro for typed access, ordered secondary indexes,
a dynamic document type, and rooted views — all with no storage-engine type leaking into the public API.

It is the successor to [StratoDb](https://github.com/corebreaker/stratodb): same typed model and derive surface,
but a different storage strategy — one blob per value, navigated in place,
rather than shredding every scalar into its own keyed node.

> **Status:** under active development, pre-1.0.
> The public API and on-disk format are not yet stable, and the crate is not yet published on crates.io.
> See [Project status](#project-status).

---

## Highlights

- **One blob per value, read zero-copy.** A value is serialized into a single blob laid out with offset tables;
  ArborDb's own codec (an rkyv-style but bespoke,
  unaligned reader) navigates to a field and reads it in place — no decode of the whole value, no alignment requirement.
- **A virtual filesystem.** A table is a tree of **directories** and **files**.
  Filesystem operations — `ls` / `mv` / `cp` / `mkdir` / `rm` — sit alongside `store` / `load` / `get`.
- **Stable identity.** Every node carries an opaque 16-byte `AKey` that survives renames and moves:
  `mv` relinks a name in O(1), the key is unchanged.
  Access paths are resolved by walking the directory tree, amortized by a per-table cache.
- **Two path kinds.** An `APath` (names only) addresses a whole value in the filesystem;
  a `VPath` (names plus list indices) navigates *inside* a value down to a scalar leaf.
- **Typed access.** Implement `AData` by hand, or `#[derive(AData)]` with Serde-style `#[arbor(...)]` attributes
  (rename, skip, default, custom (de)serialization, `from`/`into`/`try_from`, enum representations, generics, flatten).
- **Secondary indexes.** Named, composite, per-column ASC/DESC, optionally unique, scoped to a path pattern (`users/*`).
  The key encoding is order-preserving, so prefix queries are correct; write-time maintenance keeps them in sync.
- **Rooted views.** A view that makes every path relative to a fixed root, and scopes index queries to that subtree.
- **JSON / YAML export.** Render a stored value — or an in-memory `Value` subtree — to JSON or YAML
  with a hand-written, dependency-free, read-only writer.
- **Transactional.** Concurrent readers, a single serialized writer, snapshot-consistent reads,
  durable-on-commit writes.
- **Opaque engine.** The underlying key-value engine never appears in the public API.

---

## Installation

Not yet on crates.io. Depend on it by git while it is under development:

```toml
[dependencies]
arbordb = { git = "https://github.com/corebreaker/arbordb", features = ["derive"] }
```

It builds on a recent stable Rust toolchain (edition 2024). See [Cargo features](#cargo-features) for the full matrix.

---

## The data model

An `ArborDb` is one database (a file, or in-memory) holding any number of named **tables**.
A table is a tree of two node kinds:

| Node          | Holds                                                       | Identity |
|---------------|-------------------------------------------------------------|----------|
| **Directory** | a name-sorted map of children                               | `AKey`   |
| **File**      | one `Value` (an object/list/scalar tree, as a single blob)  | `AKey`   |

The fixed root is `AKey::ROOT`. An **access path** (`APath`) is a slash-separated address of names, e.g. `users/alice`.
Access paths are never persisted; they resolve by walking directories from the root (amortized by a per-table cache),
which is why a value's identity follows its key, not its location.

A file's value is **never shredded** — it is one blob. Navigation *inside* a value, down to a scalar,
uses a `VPath` (`home/city`, `tags[2]`), read zero-copy over the blob.

---

## Quick start

`create_in_memory()` keeps everything in RAM;
for a persistent file use `ArborDb::create(path)` (or `ArborDb::open(path)` to reopen one).

```rust
use std::collections::BTreeMap;
use arbordb::{data::Scalar, ArborDb, Value};

fn main() -> arbordb::AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let users = db.open_table("users")?;

    // Writes are transactional: stage, then commit.
    let w = users.write()?;
    w.store_value(
        "alice",
        &Value::Node(BTreeMap::from([(String::from("age"), Value::Leaf(Scalar::I64(30)))])),
    )?;
    w.commit()?;

    // Reads see committed data; a field is reached by its intra-value path.
    let r = users.read()?;
    assert_eq!(r.get_as::<i64>("alice", "age")?, Some(30));
    Ok(())
}
```

---

## Typed data with `#[derive(AData)]`

With the `derive` feature, a Rust type stores and loads as a whole.
The derive generates the `AData` implementation plus lazy accessors (`ArborXxx` / `ArborXxxMut`)
and an `ArborXxxDesc` companion.

```rust
use arbordb::{AData, ArborDb};

#[derive(AData, Debug, PartialEq)]
struct User {
    name: String,
    age:  u32,
}

fn main() -> arbordb::AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let users = db.open_table("users")?;

    let w = users.write()?;
    w.store::<User>("alice", &User { name: String::from("Alice"), age: 30 })?;
    w.commit()?;

    let r = users.read()?;
    assert_eq!(r.load::<User>("alice")?, Some(User { name: String::from("Alice"), age: 30 }));
    Ok(())
}
```

### `#[arbor(...)]` attributes

The derive supports a Serde-style attribute set under the `arbor` namespace:

- **Naming:** `rename = "..."`, `rename_all = "camelCase"` (the eight casings), `alias = "..."` (load-only).
- **Presence:** `skip`, `skip_store`, `skip_load`, `skip_store_if = "path"`, `default` / `default = "path"`.
- **Custom (de)serialization:** `with = "module"`, `store_with = "path"`, `load_with = "path"`.
- **Whole-type conversion:** `from = "T"`, `into = "T"`, `try_from = "T"`.
- **Enum representation:** external (default), `tag = "..."` (internal), `tag` + `content` (adjacent), `untagged`, 
  plus a variant `other` catch-all and `expecting = "..."`.
- **Generics:** supported, with an optional `bound = "..."`.
- **Flatten:** `flatten` merges a field's object into the parent node.

---

## Secondary indexes

Declare indexes on a derived type and register them in one call, or build an `IndexDef` by hand.
An index is named, has one or more columns (each ASC or DESC), may be unique,
and is scoped to a path pattern selecting the indexed entities.

```rust
use arbordb::{AData, ArborDb};

#[derive(AData, Debug, PartialEq)]
#[arbor(index(name = "by_age", columns(age)))]
struct User {
    name: String,
    age:  i64,
}

fn main() -> arbordb::AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let users = db.open_table("users")?;

    // Create + back-fill every index the type declares, scoped to `users/*`.
    users.create_indexes::<User>("users/*")?;

    let w = users.write()?;
    w.store::<User>("users/alice", &User { name: String::from("Alice"), age: 30 })?;
    w.store::<User>("users/bob", &User { name: String::from("Bob"), age: 40 })?;
    w.commit()?;

    // Query in index order (ascending by age); an empty prefix matches all.
    let r = users.read()?;
    let ages: Vec<i64> = r.find::<User>("by_age", &[])?.into_iter().map(|u| u.age).collect();
    assert_eq!(ages, [30, 40]);
    Ok(())
}
```

The encoding is order-preserving across types, byte lengths and sign,
so a prefix on a composite index returns every match ordered by the trailing columns,
and reversing a query walks the index backward.
Maintenance is bracketed around every mutation (delete affected entries → apply → re-insert),
so a whole-value replace stays consistent even under a unique index.

---

## Rooted views

`txn.rooted(root)` returns a view that interprets every path relative to a fixed root,
and scopes index queries to that subtree. Views nest.

```rust
# use arbordb::ArborDb;
# fn main() -> arbordb::AdbResult<()> {
# let db = ArborDb::create_in_memory()?;
# let table = db.open_table("t")?;
let r = table.read()?;
let alice = r.rooted("users/alice")?;
let age: Option<i64> = alice.get_as("", "age")?; // reads users/alice's `age`
# let _ = age;
# Ok(())
# }
```

---

## JSON / YAML export

ArborDb renders a stored value — or an in-memory `Value` subtree — to **JSON or YAML** with a hand-written,
dependency-free writer (no `serde_json` / `serde_yaml`, mirroring the bespoke value codec).
It is **read-only and one-directional**: there is no import path, and no cargo feature is required.

The `JsonExporter` / `YamlExporter` traits are implemented for `ReadTxn` and `RootedRead`,
which render the `Value` stored at an `APath` (a directory has no value of its own, so it is refused),
and for `Value`, which renders the in-memory subtree at a `VPath`. Object fields come out in sorted order;
JSON takes an optional indent (`None` compact, `Some(n)` pretty). Scalars with no native JSON/YAML form take a
textual one — Base64 for raw bytes, ISO 8601 / RFC 3339 for dates and times, decimal seconds for a duration,
`null` for a non-finite float.

```rust
use std::collections::BTreeMap;
use arbordb::{data::Scalar, export::{JsonExporter, YamlExporter}, ArborDb, Value};

fn main() -> arbordb::AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let table = db.open_table("people")?;

    let w = table.write()?;
    w.store_value(
        "users/alice",
        &Value::Node(BTreeMap::from([
            (String::from("name"), Value::Leaf(Scalar::Str(String::from("Alice")))),
            (String::from("age"),  Value::Leaf(Scalar::U32(30))),
        ])),
    )?;
    w.commit()?;

    let r = table.read()?;

    // Compact JSON, pretty JSON, and YAML for the value stored at an access path.
    assert_eq!(r.export_to_json("users/alice", None)?, r#"{"age":30,"name":"Alice"}"#);
    println!("{}", r.export_to_json("users/alice", Some(2))?);
    print!("{}", r.export_to_yaml("users/alice")?);

    // An in-memory `Value` renders a subtree navigated by a `VPath`.
    let value = r.load_value("users/alice")?.unwrap();
    assert_eq!(value.export_to_json("name", None)?, r#""Alice""#);
    Ok(())
}
```

See [`examples/export.rs`](crates/arbordb/examples/export.rs).

---

## Entry timestamps

With the `entry-timestamps` feature,
every file and directory carries `created` / `modified` / `accessed` datetimes (POSIX ctime/mtime/atime style).
They live **out-of-band** in a reserved side table,
so the data-table format is unchanged — a binary built *without* the feature opens the same database
and simply ignores them. Access times are **deferred**: a read buffers the time in memory,
and it becomes durable on the next committed write or an explicit `flush_access_times()`,
so a read burst never turns into a write burst.

```rust
let r = table.read()?;
if let Some(times) = r.times("users/alice")? {
    println!("created {}, modified {}", times.created(), times.modified());
}
```

See [`examples/timestamps.rs`](crates/arbordb/examples/timestamps.rs).

---

## Permissions & integrity

The `permissions` feature (which turns on `entry-timestamps`) adds user/password authentication,
per-vnode access control, and tamper detection. A database starts unprotected;
`change_password` on an unprotected handle **promotes** it — minting the master user, a per-user keyring, 
a random database integrity key, and an Ed25519 signing keypair — and authenticates the caller as master.

- **Authentication.** Passwords are never stored.
  The database secrets (the integrity key `K`
  and the private signing seed) are wrapped together (XChaCha20-Poly1305)
  under an Argon2id key derived from each user's password; authenticating *is* unwrapping them,
  so a wrong password fails the AEAD tag.
  `open_with_authentication(path, user, password)` opens and authenticates in one step;
  opening a protected database without credentials yields a read-only **guest**
  (`is_readonly` reports whether a handle can write).
- **Access control.** Every vnode has an owner, any number of groups, and a single *graded* right —
  `None` ⊂ `Access` ⊂ `Modify` ⊂ `Delete` — for its owner, for each of its groups, and for everyone else.
  `Access` reads a file / lists a directory / traverses a directory (there is no separate `walk` right);
  `Modify` adds overwriting a value or an ACL and adding/removing/renaming directory children;
  `Delete` adds removing the vnode (a cascade needs `Delete` on every descendant). A caller in several of a vnode's
  groups gets the strongest grade any of them is granted. Read a class's grade with `get_acl(path, class)`
  (`class` being `AclClass::{User, Group(name), Other}`) and set it with `set_acl(path, class, rights)`;
  `add_group` / `del_group` manage which groups a vnode is in, and `owner` / `groups` report its identity.
  Changing an ACL needs `Modify` on the vnode — there is no separate admin right.
  The **master user** bypasses all ACLs;
  the **master group** administers users and groups (without bypassing value ACLs);
  the **guest** is strictly read-only (it may only `Access`).
- **Integrity.** Every value carries two tamper tags over the same bytes — the value *and* its ACL:
  a keyed BLAKE3 MAC an authenticated reader verifies (fast, unforgeable without `K`),
  and an Ed25519 signature a keyless **guest** verifies with the public key (stored in the clear).
  So even an unauthenticated read detects a value edited by a program that bypasses ArborDb to touch the raw file,
  returning `AdbError::Tampered`; only an authenticated user holds the private seed,
  so only it can seal (write) a value.
  The user/group store carries a MAC bound to a monotonic epoch *and* the public key,
  catching rollback or a key swap at authentication. Writes verify before they trust or re-seal,
  so tampering cannot be laundered into a fresh valid tag. One limit: a guest holds no secret,
  so on its own it cannot detect an attacker that swaps the public key
  *and* re-signs a value — its check then covers only accidental corruption and naive tampering,
  while an authenticated user is covered fully. **A default guest read is therefore not trustworthy.**
  To read as a guest safely, save the public key while the database is trusted (`db.pubkey()?.write_key(path)`)
  and later **pin** it with `with_pubkey`, so verification uses the trusted key instead of the stored one
  and a swap is caught (`PublicKey` is also `AData` / `AValue` / Serde,
  so it can be stored or transmitted however you like).
  Pinning is optional: a database that never exports its key is still fully write-protected (a guest cannot write),
  but its guest reads should be treated as untrusted — reading as a guest is recommended only with a pinned key.
- **Administration.** `add_user` / `add_group` / `rename_*` / `assign_user_to_group`
  / `remove_user` (cascades the values it owns) / `remove_group` (unassigns it and strips it from every ACL); per-vnode
  `chown` / `set_acl` / `add_group` / `del_group` / `get_acl` / `owner` / `groups`.

A binary built *without* `permissions` refuses to open a protected database (`AdbError::DatabaseProtected`);
one built without `entry-timestamps` opens a timestamped database and ignores the timestamps.

See [`examples/permissions.rs`](crates/arbordb/examples/permissions.rs).

---

## Big numbers

The optional `bignum` features add arbitrary-precision `BigInt`, fixed-precision `BigFloat`,
and rational `BigRational` support. Each type can be stored either as a native `Scalar` variant (`*-as-scalar`)
or as composite data via a `Bytes` leaf (`*-as-data`); the index encoding orders them correctly,
including across byte-length and sign boundaries.

---

## Cargo features

| Feature              | Pulls in                                                                                 | Effect                                                                                                                                |
|----------------------|------------------------------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------|
| `derive`             | `arbordb-derive`                                                                         | `#[derive(AData)]` and the `#[arbor(...)]` attributes                                                                                 |
| `parallel`           | `rayon`                                                                                  | parallelize batch operations                                                                                                          |
| `serde`              | `serde`                                                                                  | `Serialize` / `Deserialize` for public types and `Value`; `store_serde_value` / `load_serde_value` (straight to/from the value codec) |
| `entry-timestamps`   | `chrono`                                                                                 | per-vnode `created` / `modified` / `accessed` datetimes (out-of-band, ignorable)                                                      |
| `permissions`        | `entry-timestamps`, `argon2`, `chacha20poly1305`, `blake3`, `ed25519-dalek`, `getrandom` | user/password auth, per-vnode ACLs, MAC + signature tamper detection                                                                  |
| `bignum`             | both umbrellas below                                                                     | every big-number type, as scalar **and** data                                                                                         |
| `bignum-as-scalar`   | the three `*-as-scalar`                                                                  | big-number `Scalar` variants + `AValue`                                                                                               |
| `bignum-as-data`     | the three `*-as-data`                                                                    | big-number `AData` impls (a `Bytes` leaf when not also a scalar)                                                                      |
| `bigint-as-scalar`   | the matching `num-*` crate                                                               | one type as a `Scalar`                                                                                                                |
| `bigfloat-as-scalar` | the matching `num-*` crate                                                               | one type as a `Scalar`                                                                                                                |
| `rational-as-scalar` | the matching `num-*` crate                                                               | one type as a `Scalar`                                                                                                                |
| `bigint-as-data`     | the matching `num-*` crate                                                               | one type as `AData`                                                                                                                   |
| `bigfloat-as-data`   | the matching `num-*` crate                                                               | one type as `AData`                                                                                                                   |
| `rational-as-data`   | the matching `num-*` crate                                                               | one type as `AData`                                                                                                                   |

Nothing is on by default.

---

## Transactions & concurrency

- `table.read()` opens a snapshot-consistent read transaction; many may run concurrently
  with each other and with a writer.
- `table.write()` opens the single active write transaction; changes are visible to that transaction immediately
  and become durable — and visible to new readers — only on `commit()`. Dropping it discards them.
- Index maintenance is bracketed around every mutation, so a whole-value replace is safe even under unique indexes.

---

## Project status

| Area                                                                        | State |
|-----------------------------------------------------------------------------|:-----:|
| Core store (virtual FS, zero-copy value codec, paths, transactions, caches) |   ✅   |
| `AData` trait, accessors, container types                                   |   ✅   |
| `#[derive(AData)]` + the full `#[arbor(...)]` attribute set                 |   ✅   |
| Secondary indexes (composite, unique, ordered, back-filled, queryable)      |   ✅   |
| Big-number scalar/data feature matrix (`bignum`)                            |   ✅   |
| Rooted views                                                                |   ✅   |
| Dynamic `Value` document type                                               |   ✅   |
| Entry timestamps (`entry-timestamps`)                                       |   ✅   |
| Permissions, ACLs, MAC + signature integrity (`permissions`)                |   ✅   |
| Documentation, runnable examples                                            |   ✅   |
| Criterion benchmark suite                                                   |   ✅   |
| Continuous integration                                                      |   ✅   |
| JSON / YAML export (read-only)                                              |   ✅   |

ArborDb is under active development:
the capabilities marked ✅ are implemented and tested, but the on-disk format and public API are not yet stable
and the crate is not yet released.

---

## License

Licensed under the [MIT License](LICENSE).
