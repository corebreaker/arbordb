//! Cross-feature integration: a single derived entity travels through the typed
//! API, a secondary index, the dynamic `Value`, an enum representation, and (with
//! `bignum`) a big-number-keyed index — the seams the per-feature suites each
//! exercise in isolation.
//!
//! Gated on `derive`; the big-number module additionally needs the `bignum`
//! family. Run the whole thing with `cargo test --all-features`.

#![cfg(feature = "derive")]

use arbordb::{data::Scalar, AData, ArborDb, Table, Value};

/// A camelCase-renamed struct carrying an adjacently-tagged enum field and a unique
/// index — several seams at once.
#[derive(AData, Debug, PartialEq)]
#[arbor(rename_all = "camelCase", index(name = "by_email", columns(email), unique))]
struct Member {
    full_name: String,
    team:      String,
    email:     String,
    role:      Role,
}

/// An adjacently-tagged enum: a unit variant and a struct variant.
#[derive(AData, Debug, PartialEq)]
#[arbor(tag = "kind", content = "data")]
enum Role {
    Member,
    Lead { reports: u32 },
}

fn alice() -> Member {
    Member {
        full_name: String::from("Alice Eng"),
        team:      String::from("eng"),
        email:     String::from("alice@example.io"),
        role:      Role::Lead {
            reports: 3
        },
    }
}

fn seed(table: &Table) {
    let w = table.write().unwrap();
    w.store::<Member>("members/alice", &alice()).unwrap();
    w.store::<Member>(
        "members/bob",
        &Member {
            full_name: String::from("Bob Eng"),
            team:      String::from("eng"),
            email:     String::from("bob@example.io"),
            role:      Role::Member,
        },
    )
    .unwrap();
    w.commit().unwrap();
}

#[test]
fn derived_entity_indexes_queries_and_recomposes() {
    let db = ArborDb::create_in_memory().unwrap();
    let members = db.open_table("members").unwrap();

    // The unique index `Member` declares, scoped to `members/*`, back-filled.
    members.create_indexes::<Member>("members/*").unwrap();
    seed(&members);

    let r = members.read().unwrap();

    // A query recomposes the full entity from its blob — including the enum field.
    let found: Vec<Member> = r
        .find("by_email", &[Scalar::Str(String::from("alice@example.io"))])
        .unwrap();
    assert_eq!(found, vec![alice()]);

    // Fields are stored under their camelCase names, never the Rust identifiers.
    assert!(r.get("members/alice", "fullName").unwrap().is_some());
    assert!(r.get("members/alice", "full_name").unwrap().is_none());

    // The unique index rejects a second entity with an existing email; the failed
    // write is discarded by dropping the (uncommitted) transaction.
    let w = members.write().unwrap();
    let duplicate = w.store::<Member>(
        "members/dave",
        &Member {
            full_name: String::from("Dave"),
            team:      String::from("eng"),
            email:     String::from("alice@example.io"),
            role:      Role::Member,
        },
    );
    assert!(matches!(duplicate, Err(arbordb::AdbError::UniqueViolation { .. })));
    drop(w);
}

#[test]
fn derived_entity_round_trips_through_a_dynamic_value() {
    let db = ArborDb::create_in_memory().unwrap();
    let members = db.open_table("members").unwrap();
    seed(&members);

    let r = members.read().unwrap();

    // The stored subtree loads into a faithful `Value`, addressable by intra-value
    // path — camelCase field names and the adjacently-tagged enum included.
    let value = r.load_value("members/alice").unwrap().unwrap();
    assert!(matches!(value, Value::Node(_)));
    assert_eq!(
        value.get_value("fullName"),
        Some(Value::Leaf(Scalar::Str("Alice Eng".into())))
    );
    assert_eq!(
        value.get_value("role/kind"),
        Some(Value::Leaf(Scalar::Str("Lead".into())))
    );
    assert_eq!(value.get_value("role/data/reports"), Some(Value::Leaf(Scalar::U32(3))));

    // Stored back through the dynamic path, it recomposes into the same typed entity.
    {
        let w = members.write().unwrap();
        w.store_value("clones/alice", &value).unwrap();
        w.commit().unwrap();
    }

    let r = members.read().unwrap();
    assert_eq!(r.load::<Member>("clones/alice").unwrap(), Some(alice()));
}

/// Big-number scalars flowing through a derived entity and its index. Needs the
/// `bignum` family on top of `derive` (so `--all-features`).
#[cfg(feature = "bignum")]
mod bignum {
    use super::*;
    use num_bigint::BigInt;

    #[derive(AData, Debug, PartialEq)]
    #[arbor(index(name = "by_balance", columns(balance)))]
    struct Account {
        owner:   String,
        balance: BigInt,
    }

    #[test]
    fn a_bigint_keyed_index_orders_by_value_across_byte_lengths_and_sign() {
        let db = ArborDb::create_in_memory().unwrap();
        let accounts = db.open_table("accounts").unwrap();
        accounts.create_indexes::<Account>("accounts/*").unwrap();

        // Insertion order is scrambled. The index must return ascending by value,
        // including across the one-byte/two-byte magnitude boundary (127 vs 128) and
        // across the sign — exactly where a fixed-width encoding would misorder.
        let balances = [
            BigInt::from(128),
            BigInt::from(-1_000_000),
            BigInt::from(0),
            BigInt::from(127),
            BigInt::from(-1),
            BigInt::from(1_000_000),
        ];

        {
            let w = accounts.write().unwrap();
            for (i, balance) in balances.iter().enumerate() {
                w.store::<Account>(
                    &format!("accounts/a{i}"),
                    &Account {
                        owner:   format!("owner{i}"),
                        balance: balance.clone(),
                    },
                )
                .unwrap();
            }
            w.commit().unwrap();
        }

        // An empty prefix matches every indexed entity, in ascending index order.
        let r = accounts.read().unwrap();
        let got: Vec<BigInt> = r
            .find::<Account>("by_balance", &[])
            .unwrap()
            .into_iter()
            .map(|account| account.balance)
            .collect();

        let mut want = balances.to_vec();
        want.sort();
        assert_eq!(got, want);
    }
}

/// Secondary indexes crossed with `permissions`: an index query must not leak
/// entities the authenticated reader is not allowed to see. Needs `derive` +
/// `permissions` (so `--all-features`).
#[cfg(feature = "permissions")]
mod permissions {
    use super::*;
    use arbordb::acl::{Mode, Rights};

    #[derive(AData, Debug, PartialEq)]
    #[arbor(index(name = "by_rank", columns(rank)))]
    struct Doc {
        title: String,
        rank:  i64,
    }

    #[test]
    fn an_index_query_hides_entities_the_reader_cannot_see() {
        // Promote to master (who bypasses ACLs), then add an ordinary user.
        let master = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
        master.add_user("alice", "a", false).unwrap();

        let docs = master.open_table("docs").unwrap();
        docs.create_indexes::<Doc>("docs/*").unwrap();

        {
            let w = docs.write().unwrap();
            w.store::<Doc>(
                "docs/public",
                &Doc {
                    title: String::from("Public"),
                    rank:  1,
                },
            )
            .unwrap();
            w.store::<Doc>(
                "docs/secret",
                &Doc {
                    title: String::from("Secret"),
                    rank:  2,
                },
            )
            .unwrap();

            // Lock `secret` down to its owner (master); other gets nothing.
            w.chmod(
                "docs/secret",
                Mode {
                    owner: Rights {
                        read:  true,
                        write: true,
                        walk:  true,
                    },
                    group: Rights::default(),
                    other: Rights::default(),
                },
            )
            .unwrap();
            w.commit().unwrap();
        }

        // The master sees both, in index order.
        {
            let r = docs.read().unwrap();
            let ranks: Vec<i64> = r
                .find::<Doc>("by_rank", &[])
                .unwrap()
                .into_iter()
                .map(|doc| doc.rank)
                .collect();
            assert_eq!(ranks, [1, 2]);
        }

        // `alice` is "other": she may read `public` but not `secret`. The index query
        // silently omits the entity she cannot read rather than leaking it.
        {
            let alice = master.clone().with_authentication("alice", "a").unwrap();
            let r = alice.open_table("docs").unwrap().read().unwrap();
            let titles: Vec<String> = r
                .find::<Doc>("by_rank", &[])
                .unwrap()
                .into_iter()
                .map(|doc| doc.title)
                .collect();
            assert_eq!(titles, [String::from("Public")]);
        }
    }
}
