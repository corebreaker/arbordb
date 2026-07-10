//! The typed data contract: scalars as `AData`, and `store`/`load`/`fetch`.
//!
//! Composite types (structs/enums) arrive with the containers and the
//! `#[derive(AData)]` macro; here the contract is exercised on scalar leaves,
//! which implement `AData` as a single-leaf value.

use arbordb::{
    data::{Leaf, LeafMut},
    ArborDb,
};

#[test]
fn stores_and_loads_typed_scalars() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<i64>("nums/answer", &42).unwrap();
        w.store::<String>("who/name", &String::from("Alice")).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<i64>("nums/answer").unwrap(), Some(42));
    assert_eq!(r.load::<String>("who/name").unwrap(), Some(String::from("Alice")));
    assert_eq!(r.load::<i64>("nums/missing").unwrap(), None);
}

#[test]
fn fetches_a_scalar_accessor_that_outlives_the_txn() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<i64>("answer", &42).unwrap();
        w.commit().unwrap();
    }

    let leaf: Leaf<'static, i64> = {
        let r = table.read().unwrap();

        r.fetch("answer").unwrap().unwrap()
    };

    // The accessor owns its blob snapshot, so it reads fine after the read
    // transaction is gone.
    assert_eq!(leaf.get().unwrap(), 42);
}

#[test]
fn edits_a_scalar_through_fetch_mut() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<i64>("counter", &1).unwrap();
        w.commit().unwrap();
    }

    {
        let w = table.write().unwrap();
        {
            let counter: LeafMut<'_, i64> = w.fetch_mut("counter").unwrap().unwrap();
            assert_eq!(counter.get().unwrap(), 1);
            counter.set(&2).unwrap();
            assert_eq!(counter.get().unwrap(), 2); // reads its own uncommitted edit
        } // the write accessor borrows the txn, so drop it before commit

        // `fetch_mut` on an absent file yields nothing.
        assert!(w.fetch_mut::<LeafMut<'_, i64>>("missing").unwrap().is_none());
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.load::<i64>("counter").unwrap(), Some(2));
}
