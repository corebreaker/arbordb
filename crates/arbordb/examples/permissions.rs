//! User authentication and per-vnode access control with the `permissions` feature.
//!
//! A fresh database is unprotected. Promoting it (via `change_password`) mints a
//! master user, a per-user keyring, and a database integrity key; from then on
//! every value carries a keyed MAC and every access is ACL-checked. This example
//! promotes a database, adds a second user, and shows the owner/other split.
//!
//! Run with: `cargo run --example permissions --features permissions`

use arbordb::{
    acl::{Mode, Rights},
    data::Scalar,
    AdbError,
    AdbResult,
    ArborDb,
    Value,
};

use std::collections::BTreeMap;

/// A one-field object, so `get_as(path, "n")` reads its scalar.
fn note(n: i64) -> Value {
    Value::Node(BTreeMap::from([(String::from("n"), Value::Leaf(Scalar::I64(n)))]))
}

fn main() -> AdbResult<()> {
    // A fresh database has no permission system. `change_password` *promotes* it and
    // authenticates this handle as the master user, who bypasses every ACL.
    let master = ArborDb::create_in_memory()?.change_password("master-secret")?;
    println!("promoted; current user = {:?}", master.current_user());

    // The master registers a second user (and, with `true`, a same-named group).
    master.add_user("alice", "alice-secret", true)?;
    println!("users = {:?}", master.list_users()?);

    // The master stores two files. A freshly created file gets a default ACL: the
    // owner gets every right, group and other get read + walk.
    {
        let docs = master.open_table("docs")?;
        let w = docs.write()?;
        w.store_value("shared/welcome", &note(1))?;
        w.store_value("vault/secret", &note(42))?;

        // Lock the vault down to its owner (master) alone: no group, no other.
        w.chmod(
            "vault/secret",
            Mode {
                owner: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                group: Rights::default(),
                other: Rights::default(),
            },
        )?;
        w.commit()?;
    }

    // Re-authenticate the same (in-memory) database as `alice` — a separate handle
    // over the same store, so `master` stays valid alongside it.
    let alice = master.clone().with_authentication("alice", "alice-secret")?;
    {
        let r = alice.open_table("docs")?.read()?;

        // `alice` owns neither file and is in neither file's group, so she is "other".
        // `shared/welcome` keeps the default other-read, so she may read it.
        println!(
            "alice reads shared/welcome n = {:?}",
            r.get_as::<i64>("shared/welcome", "n")?
        );

        // `vault/secret` grants nothing to other, so the read is denied.
        match r.get_as::<i64>("vault/secret", "n") {
            Err(AdbError::PermissionDenied(_)) => println!("alice is denied vault/secret (as expected)"),
            other => println!("unexpected result for alice on vault/secret: {other:?}"),
        }
    }

    // The master (or any reader that may see it) can inspect the resolved ACL and the
    // vnode's timestamps (the `permissions` feature implies `entry-timestamps`).
    let r = master.open_table("docs")?.read()?;
    if let Some(acl) = r.get_acl("vault/secret")? {
        println!(
            "vault/secret: owner={}, group={:?}, other.read={}",
            acl.owner, acl.group, acl.mode.other.read,
        );
    }
    if let Some(times) = r.times("vault/secret")? {
        println!("vault/secret created at {}", times.created());
    }

    Ok(())
}
