//! Integration tests for the `permissions` feature: authentication, protection
//! promotion, and password changes. (ACL enforcement is exercised separately.)

#![cfg(feature = "permissions")]

use arbordb::{
    acl::{AclClass, Rights},
    data::{AValue, Scalar},
    perm::PublicKey,
    AdbError,
    ArborDb,
    Value,
};

use std::path::PathBuf;

fn leaf(n: i64) -> Value {
    Value::Leaf(Scalar::I64(n))
}

/// A throwaway on-disk database path (in-memory databases cannot be reopened, so
/// the authentication matrix needs a real file).
fn tmp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.redb");

    (dir, path)
}

#[test]
fn rooted_views_forward_acl_reads_relative_to_the_root() {
    let master = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
    let table = master.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store_value("dir/file", &leaf(1)).unwrap();
        w.set_acl("dir/file", AclClass::Other, Rights::Access).unwrap();

        // The rooted write view reads the same ACL, owner, and groups as the
        // absolute path, on this transaction's own uncommitted state.
        let dir = w.rooted("dir").unwrap();
        assert_eq!(
            dir.get_acl("file", AclClass::Other).unwrap(),
            w.get_acl("dir/file", AclClass::Other).unwrap()
        );

        assert_eq!(dir.owner("file").unwrap(), w.owner("dir/file").unwrap());
        assert_eq!(dir.groups("file").unwrap(), w.groups("dir/file").unwrap());

        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    let dir = r.rooted("dir").unwrap();

    assert_eq!(dir.get_acl("file", AclClass::Other).unwrap(), Rights::Access);
    assert_eq!(dir.owner("file").unwrap(), Some(String::from("master")));
    assert_eq!(
        dir.get_acl("file", AclClass::User).unwrap(),
        r.get_acl("dir/file", AclClass::User).unwrap()
    );

    assert_eq!(dir.groups("file").unwrap(), r.groups("dir/file").unwrap());
}

#[test]
fn change_password_promotes_an_unprotected_database_to_master() {
    let (_dir, path) = tmp_db();

    // A fresh database has no permission system.
    let db = ArborDb::create(&path).unwrap();
    assert_eq!(db.current_user(), None);

    // Promotion makes the caller the master user.
    let db = db.change_password("s3cret").unwrap();
    assert_eq!(db.current_user(), Some("master"));
    drop(db);

    // Reopening without credentials lands on the guest user.
    let guest = ArborDb::open(&path).unwrap();
    assert_eq!(guest.current_user(), Some("guest"));
}

#[test]
fn open_with_authentication_accepts_master_and_rejects_bad_credentials() {
    let (_dir, path) = tmp_db();
    ArborDb::create(&path).unwrap().change_password("pw").unwrap();

    // redb locks the file exclusively, so release this handle before reopening.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
        assert_eq!(master.current_user(), Some("master"));
    }

    // A wrong password and an unknown user are indistinguishable failures.
    assert!(matches!(
        ArborDb::open_with_authentication(&path, "master", "nope"),
        Err(AdbError::AuthenticationFailed)
    ));
    assert!(matches!(
        ArborDb::open_with_authentication(&path, "ghost", "pw"),
        Err(AdbError::AuthenticationFailed)
    ));
}

#[test]
fn with_authentication_on_an_unprotected_database_reports_no_permissions() {
    let (_dir, path) = tmp_db();
    let db = ArborDb::create(&path).unwrap();

    assert!(matches!(
        db.with_authentication("master", "pw"),
        Err(AdbError::NoPermissions)
    ));
}

#[test]
fn guest_cannot_change_a_password() {
    let (_dir, path) = tmp_db();
    ArborDb::create(&path).unwrap().change_password("pw").unwrap();

    let guest = ArborDb::open(&path).unwrap();
    assert!(matches!(guest.change_password("x"), Err(AdbError::PermissionDenied(_))));
}

#[test]
fn a_user_changes_their_own_password() {
    let (_dir, path) = tmp_db();
    ArborDb::create(&path).unwrap().change_password("old").unwrap();

    // Authenticate as master, then rotate the password.
    ArborDb::open_with_authentication(&path, "master", "old")
        .unwrap()
        .change_password("new")
        .unwrap();

    // The old password no longer works; the new one does.
    assert!(matches!(
        ArborDb::open_with_authentication(&path, "master", "old"),
        Err(AdbError::AuthenticationFailed)
    ));
    assert_eq!(
        ArborDb::open_with_authentication(&path, "master", "new")
            .unwrap()
            .current_user(),
        Some("master")
    );
}

#[test]
fn guest_cannot_write_but_master_can() {
    let (_dir, path) = tmp_db();

    // Promote to master and write a file (master bypasses ACLs).
    {
        let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("greeting", &leaf(1)).unwrap();
        w.commit().unwrap();
    }

    // Guest (the anonymous open) is read-only: it cannot even open a write txn.
    {
        let guest = ArborDb::open(&path).unwrap();
        let t = guest.open_table("data").unwrap();
        assert!(matches!(t.write(), Err(AdbError::PermissionDenied(_))));
    }

    // The re-authenticated master may overwrite.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
        let t = master.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("greeting", &leaf(3)).unwrap();
        w.commit().unwrap();
    }
}

#[test]
fn guest_can_read_a_default_readable_file() {
    let (_dir, path) = tmp_db();

    // Master creates a nested file; default ACLs grant `other` the Access grade.
    {
        let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("docs/readme", &leaf(42)).unwrap();
        w.commit().unwrap();
    }

    // Guest traverses the directories and reads the file (all Access-level).
    {
        let guest = ArborDb::open(&path).unwrap();
        let t = guest.open_table("data").unwrap();
        assert_eq!(t.read().unwrap().load_value("docs/readme").unwrap(), Some(leaf(42)));
    }
}

#[test]
fn an_unprotected_database_writes_without_restriction() {
    // A database with no permission system authorizes every operation.
    let db = ArborDb::create_in_memory().unwrap();
    let t = db.open_table("data").unwrap();

    let w = t.write().unwrap();
    w.store_value("x", &leaf(1)).unwrap();
    w.commit().unwrap();

    assert_eq!(t.read().unwrap().load_value("x").unwrap(), Some(leaf(1)));
}

#[test]
fn owner_may_write_but_another_user_only_reads() {
    let (_dir, path) = tmp_db();

    // Master promotes and creates two ordinary users.
    {
        let db = ArborDb::create(&path).unwrap().change_password("master-pw").unwrap();
        db.add_user("alice", "alice-pw", false).unwrap();
        db.add_user("bob", "bob-pw", false).unwrap();
    }

    // Alice creates a file — she owns it (default: owner Delete, other Access).
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "alice-pw").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("shared", &leaf(1)).unwrap();
        w.commit().unwrap();
    }

    // Bob falls in the `other` class: he may read Alice's file but not overwrite it.
    {
        let bob = ArborDb::open_with_authentication(&path, "bob", "bob-pw").unwrap();
        let t = bob.open_table("data").unwrap();
        assert_eq!(t.read().unwrap().load_value("shared").unwrap(), Some(leaf(1)));

        let w = t.write().unwrap();
        assert!(matches!(
            w.store_value("shared", &leaf(2)),
            Err(AdbError::PermissionDenied(_))
        ));
    }

    // Alice, the owner, may overwrite it.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "alice-pw").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("shared", &leaf(3)).unwrap();
        w.commit().unwrap();
    }
}

#[test]
fn chown_transfers_ownership() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        db.add_user("alice", "a", false).unwrap();
        db.add_user("bob", "b", false).unwrap();
    }

    // Alice creates a file she owns, then hands it to Bob (she is the owner).
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(1)).unwrap();
        w.chown("f", "bob").unwrap();
        w.commit().unwrap();
    }

    // Bob is now the owner and may write it.
    {
        let bob = ArborDb::open_with_authentication(&path, "bob", "b").unwrap();
        let t = bob.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(2)).unwrap();
        w.commit().unwrap();
    }

    // Alice is now merely `other` and may no longer write it.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        assert!(matches!(
            w.store_value("f", &leaf(3)),
            Err(AdbError::PermissionDenied(_))
        ));
    }
}

#[test]
fn add_and_del_group_manage_a_nodes_groups() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        db.add_user("alice", "a", false).unwrap();
        db.add_user("bob", "b", false).unwrap();
        db.add_group("staff").unwrap();
    }

    // Alice owns the file, so she may add a group to it and set that group's grade.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(1)).unwrap();
        w.add_group("f", "staff").unwrap();
        w.set_acl("f", AclClass::Group(String::from("staff")), Rights::Modify)
            .unwrap();
        w.commit().unwrap();

        // `groups` lists it and `get_acl` reports the grade it was granted.
        let r = t.read().unwrap();
        assert_eq!(r.groups("f").unwrap(), vec![String::from("staff")]);
        assert_eq!(
            r.get_acl("f", AclClass::Group(String::from("staff"))).unwrap(),
            Rights::Modify
        );
    }

    // Bob is only `other` on the file, so he cannot touch its ACL.
    {
        let bob = ArborDb::open_with_authentication(&path, "bob", "b").unwrap();
        let w = bob.open_table("data").unwrap().write().unwrap();
        assert!(matches!(w.add_group("f", "staff"), Err(AdbError::PermissionDenied(_))));
    }

    // Alice removes the group again; the a-node belongs to nothing afterwards.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.del_group("f", "staff").unwrap();
        w.commit().unwrap();

        assert!(t.read().unwrap().groups("f").unwrap().is_empty());
    }
}

#[test]
fn deleting_needs_the_delete_grade_not_merely_modify() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        db.add_user("alice", "a", false).unwrap();

        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(1)).unwrap();

        // Grant `other` Modify: enough to overwrite the value, not to delete the a-node.
        w.set_acl("f", AclClass::Other, Rights::Modify).unwrap();
        w.commit().unwrap();
    }

    // Alice (an `other`) may overwrite the file, but removing it is denied.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(2)).unwrap();
        assert!(matches!(w.rm("f"), Err(AdbError::PermissionDenied(_))));
        w.commit().unwrap();
    }

    // The master raises `other` to Delete.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "m").unwrap();
        let w = master.open_table("data").unwrap().write().unwrap();
        w.set_acl("f", AclClass::Other, Rights::Delete).unwrap();
        w.commit().unwrap();
    }

    // Now Alice may remove it.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        assert!(w.rm("f").unwrap());
        w.commit().unwrap();
    }
}

#[test]
fn set_acl_restricts_access_and_get_acl_reflects_it() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("secret", &leaf(7)).unwrap();

        // Restrict to the owner alone: revoke everyone else (the default owner grade
        // is already `Delete`).
        w.set_acl("secret", AclClass::Other, Rights::None).unwrap();
        w.commit().unwrap();

        // get_acl / owner / groups report the tightened ACL.
        let r = t.read().unwrap();
        assert_eq!(r.owner("secret").unwrap(), Some(String::from("master")));
        assert!(r.groups("secret").unwrap().is_empty());
        assert_eq!(r.get_acl("secret", AclClass::User).unwrap(), Rights::Delete);
        assert_eq!(r.get_acl("secret", AclClass::Other).unwrap(), Rights::None);
    }

    // The guest (an `other`) may no longer read it.
    {
        let guest = ArborDb::open(&path).unwrap();
        let t = guest.open_table("data").unwrap();
        assert!(matches!(
            t.read().unwrap().load_value("secret"),
            Err(AdbError::PermissionDenied(_))
        ));
    }
}

#[test]
fn kind_and_exists_enforce_access_on_the_target_a_node() {
    let (_dir, path) = tmp_db();

    // Master stores a file and revokes `other` on it entirely.
    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("secret", &leaf(7)).unwrap();
        w.set_acl("secret", AclClass::Other, Rights::None).unwrap();
        w.commit().unwrap();
    }

    // The guest can traverse the root but holds no `Access` on `secret`, so it can
    // neither learn the file's kind nor probe its existence — the same denial a
    // `load` raises, so existence cannot be leaked past a `Rights::None` ACL.
    {
        let guest = ArborDb::open(&path).unwrap();
        let r = guest.open_table("data").unwrap().read().unwrap();

        assert!(matches!(r.kind("secret"), Err(AdbError::PermissionDenied(_))));
        assert!(matches!(r.exists("secret"), Err(AdbError::PermissionDenied(_))));

        // A path that genuinely does not exist still reports absence, not a denial.
        assert_eq!(r.kind("ghost").unwrap(), None);
        assert!(!r.exists("ghost").unwrap());
    }
}

#[test]
fn kind_verifies_the_target_a_nodes_integrity() {
    use redb::{Database, ReadableTable, TableDefinition};

    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
        let t = db.open_table("docs").unwrap();
        let w = t.write().unwrap();
        w.store_value("note", &leaf(42)).unwrap();
        w.commit().unwrap();
    }

    // Flip a byte inside the file's own blob; its leading tag still marks it a file,
    // so the parent directory entry stays intact and `resolve` still finds it.
    {
        let docs: TableDefinition<u128, &[u8]> = TableDefinition::new("docs");
        let raw = Database::open(&path).unwrap();
        let wtx = raw.begin_write().unwrap();
        {
            let mut table = wtx.open_table(docs).unwrap();

            let mut victim: Option<(u128, Vec<u8>)> = None;
            for row in table.iter().unwrap() {
                let (key, value) = row.unwrap();
                let bytes = value.value().to_vec();
                if bytes.first() == Some(&1u8) {
                    victim = Some((key.value(), bytes));
                }
            }

            let (key, mut bytes) = victim.expect("a file entry to tamper with");
            *bytes.last_mut().unwrap() ^= 0x01;
            table.insert(key, bytes.as_slice()).unwrap();
        }
        wtx.commit().unwrap();
    }

    // `kind` now verifies the target a-node's own tag, so it catches the tampering
    // exactly as `load_value` does, rather than reporting the kind of altered bytes.
    let master = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
    let t = master.open_table("docs").unwrap();
    let r = t.read().unwrap();
    assert!(matches!(r.kind("note"), Err(AdbError::Tampered(_))));
}

#[test]
fn removing_a_user_deletes_the_values_it_owns() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        db.add_user("alice", "a", false).unwrap();
    }

    // Alice creates a top-level file and a nested one; she owns both (and the dir).
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("a1", &leaf(1)).unwrap();
        w.store_value("dir/a2", &leaf(2)).unwrap();
        w.commit().unwrap();
    }

    // Master removes Alice: her values are reaped and she is gone from the store.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "m").unwrap();
        master.remove_user("alice").unwrap();

        assert_eq!(master.list_users().unwrap(), vec!["guest", "master"]);

        let t = master.open_table("data").unwrap();
        let r = t.read().unwrap();
        assert!(r.load_value("a1").unwrap().is_none());
        assert!(r.load_value("dir/a2").unwrap().is_none());
    }

    // The built-in users cannot be removed.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "m").unwrap();
        assert!(matches!(
            master.remove_user("master"),
            Err(AdbError::PermissionDenied(_))
        ));
        assert!(matches!(
            master.remove_user("guest"),
            Err(AdbError::PermissionDenied(_))
        ));
    }
}

#[test]
fn removing_a_group_unassigns_it_and_clears_it_from_acls() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        db.add_user("alice", "a", false).unwrap();
        db.add_group("staff").unwrap();
        db.assign_user_to_group("alice", "staff").unwrap();

        // A file grouped to "staff".
        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(1)).unwrap();
        w.set_acl("f", AclClass::Group(String::from("staff")), Rights::Access)
            .unwrap();
        w.commit().unwrap();

        assert_eq!(t.read().unwrap().groups("f").unwrap(), vec![String::from("staff")]);
    }

    // Removing the group unassigns every user and clears it from every ACL.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "m").unwrap();
        master.remove_group("staff").unwrap();

        assert!(master.user_groups("alice").unwrap().is_empty());
        assert!(!master.list_groups().unwrap().contains(&String::from("staff")));

        let t = master.open_table("data").unwrap();
        assert!(t.read().unwrap().groups("f").unwrap().is_empty());
    }

    // The built-in groups cannot be removed.
    {
        let master = ArborDb::open_with_authentication(&path, "master", "m").unwrap();
        assert!(matches!(
            master.remove_group("master"),
            Err(AdbError::PermissionDenied(_))
        ));
        assert!(matches!(
            master.remove_group("super"),
            Err(AdbError::PermissionDenied(_))
        ));
    }
}

#[test]
fn admin_manages_users_and_groups() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();

        // Create a user together with a same-named group.
        db.add_user("alice", "a", true).unwrap();

        // The master may list everything.
        assert_eq!(db.list_users().unwrap(), vec!["alice", "guest", "master"]);
        assert!(db.list_groups().unwrap().contains(&String::from("alice")));
        assert_eq!(db.user_groups("alice").unwrap(), vec!["alice"]);
        assert_eq!(db.group_members("alice").unwrap(), vec!["alice"]);

        // The frozen guest cannot be put in any group, even by the master.
        assert!(matches!(
            db.assign_user_to_group("guest", "alice"),
            Err(AdbError::Frozen(_))
        ));

        // Built-in users and groups cannot be renamed.
        assert!(matches!(
            db.rename_user("master", "boss"),
            Err(AdbError::PermissionDenied(_))
        ));
        assert!(matches!(
            db.rename_group("super", "sudo"),
            Err(AdbError::PermissionDenied(_))
        ));
    }

    // An ordinary user may neither administer nor list.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        assert!(matches!(
            alice.add_user("carol", "c", false),
            Err(AdbError::PermissionDenied(_))
        ));
        assert!(matches!(alice.list_users(), Err(AdbError::PermissionDenied(_))));
    }
}

#[test]
fn tampering_with_the_permission_store_is_detected() {
    use redb::{Database, ReadableTable, TableDefinition};

    let (_dir, path) = tmp_db();
    ArborDb::create(&path).unwrap().change_password("pw").unwrap();

    // Simulate an attacker who opens the redb file directly — no ArborDb, no key —
    // and rewrites the group blob in the reserved `$metadata` table.
    {
        let meta: TableDefinition<&str, &[u8]> = TableDefinition::new("$metadata");
        let raw = Database::open(&path).unwrap();
        let wtx = raw.begin_write().unwrap();
        {
            let mut table = wtx.open_table(meta).unwrap();
            let mut groups = table.get("groups").unwrap().unwrap().value().to_vec();
            *groups.last_mut().unwrap() ^= 0x01;
            table.insert("groups", groups.as_slice()).unwrap();
        }
        wtx.commit().unwrap();
    }

    // Authentication unlocks K, then fails the control-plane integrity check.
    assert!(matches!(
        ArborDb::open_with_authentication(&path, "master", "pw"),
        Err(AdbError::Tampered(_))
    ));
}

#[test]
fn tampering_with_a_value_is_detected() {
    use redb::{Database, ReadableTable, TableDefinition};

    let (_dir, path) = tmp_db();

    // Master stores a top-level file, then releases the handle.
    {
        let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
        let t = db.open_table("docs").unwrap();
        let w = t.write().unwrap();
        w.store_value("note", &leaf(42)).unwrap();
        w.commit().unwrap();
    }

    // An attacker flips a byte inside the file's stored blob. Its leading tag byte
    // marks it a file, so the root directory entry is left intact.
    {
        let docs: TableDefinition<u128, &[u8]> = TableDefinition::new("docs");
        let raw = Database::open(&path).unwrap();
        let wtx = raw.begin_write().unwrap();
        {
            let mut table = wtx.open_table(docs).unwrap();

            let mut victim: Option<(u128, Vec<u8>)> = None;
            for row in table.iter().unwrap() {
                let (key, value) = row.unwrap();
                let bytes = value.value().to_vec();
                if bytes.first() == Some(&1u8) {
                    victim = Some((key.value(), bytes));
                }
            }

            let (key, mut bytes) = victim.expect("a file entry to tamper with");
            *bytes.last_mut().unwrap() ^= 0x01;
            table.insert(key, bytes.as_slice()).unwrap();
        }
        wtx.commit().unwrap();
    }

    // Authentication still succeeds — the control plane is intact — but reading the
    // tampered value fails its integrity check.
    let master = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
    let t = master.open_table("docs").unwrap();
    let r = t.read().unwrap();
    assert!(matches!(r.load_value("note"), Err(AdbError::Tampered(_))));
}

#[test]
fn a_writer_cannot_launder_a_tampered_value() {
    use redb::{Database, ReadableTable, TableDefinition};

    let (_dir, path) = tmp_db();

    // Master stores a top-level file, then releases the handle.
    {
        let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
        let t = db.open_table("docs").unwrap();
        let w = t.write().unwrap();
        w.store_value("note", &leaf(7)).unwrap();
        w.commit().unwrap();
    }

    // An attacker flips a byte in the file's stored blob via redb alone.
    {
        let docs: TableDefinition<u128, &[u8]> = TableDefinition::new("docs");
        let raw = Database::open(&path).unwrap();
        let wtx = raw.begin_write().unwrap();
        {
            let mut table = wtx.open_table(docs).unwrap();

            let mut victim: Option<(u128, Vec<u8>)> = None;
            for row in table.iter().unwrap() {
                let (key, value) = row.unwrap();
                let bytes = value.value().to_vec();
                if bytes.first() == Some(&1u8) {
                    victim = Some((key.value(), bytes));
                }
            }

            let (key, mut bytes) = victim.expect("a file entry to tamper with");
            *bytes.last_mut().unwrap() ^= 0x01;
            table.insert(key, bytes.as_slice()).unwrap();
        }
        wtx.commit().unwrap();
    }

    // Copying reads the source blob to duplicate it, so the writer detects the
    // tampering (and cannot re-seal a fresh, valid MAC over the altered bytes).
    let master = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
    let t = master.open_table("docs").unwrap();
    let w = t.write().unwrap();
    assert!(matches!(w.cp("note", "copy"), Err(AdbError::Tampered(_))));
}

#[test]
fn public_key_bytes_and_file_round_trip() {
    let (dir, path) = tmp_db();

    let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
    let key = db.pubkey().unwrap();

    // Writing the key to a file then reading it back yields the same key.
    let saved = dir.path().join("trusted.pub");
    key.write_key(&saved).unwrap();
    assert_eq!(PublicKey::read_key(&saved).unwrap(), key);

    // Bytes round-trip through `as_bytes` / `into_bytes` / `from_bytes` (32 bytes).
    assert_eq!(key.as_bytes().len(), 32);
    assert_eq!(PublicKey::from_bytes(*key.as_bytes()), key);
    assert_eq!(PublicKey::from_bytes(key.into_bytes()), key);

    // A database with no permission system has no public key.
    assert!(matches!(
        ArborDb::create_in_memory().unwrap().pubkey(),
        Err(AdbError::NoPermissions)
    ));
}

#[test]
fn public_key_round_trips_as_avalue_and_adata() {
    let db = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
    let key = db.pubkey().unwrap();

    // AValue: to_scalar / from_scalar.
    assert_eq!(key.to_scalar(), Scalar::Bytes(key.as_bytes().to_vec()));
    assert_eq!(PublicKey::from_scalar(&key.to_scalar()).unwrap(), key);
    assert!(matches!(
        PublicKey::from_scalar(&Scalar::I64(1)),
        Err(AdbError::TypeMismatch { .. })
    ));

    // AData: store then load it as a stored value (master bypasses ACLs).
    let t = db.open_table("keys").unwrap();
    {
        let w = t.write().unwrap();
        w.store::<PublicKey>("mine", &key).unwrap();
        w.commit().unwrap();
    }
    assert_eq!(t.read().unwrap().load::<PublicKey>("mine").unwrap(), Some(key));
}

#[cfg(feature = "serde")]
#[test]
fn public_key_serde_round_trips() {
    let db = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
    let key = db.pubkey().unwrap();

    let json = serde_json::to_string(&key).unwrap();
    assert_eq!(serde_json::from_str::<PublicKey>(&json).unwrap(), key);
}

#[test]
fn pinning_a_trusted_public_key_drives_guest_verification() {
    let (_dir, path) = tmp_db();

    // A trusted database: master stores a world-readable value; capture its key.
    let genuine = {
        let db = ArborDb::create(&path).unwrap().change_password("pw").unwrap();
        let t = db.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("docs/readme", &leaf(42)).unwrap();
        w.commit().unwrap();

        db.pubkey().unwrap()
    };

    // A guest pinned to the genuine key reads the value — the signatures verify.
    {
        let guest = ArborDb::open(&path).unwrap().with_pubkey(genuine).unwrap();
        let t = guest.open_table("data").unwrap();
        assert_eq!(t.read().unwrap().load_value("docs/readme").unwrap(), Some(leaf(42)));
    }

    // A guest pinned to a DIFFERENT (valid) key rejects the value as tampered: the
    // pinned key, not the stored one, drives verification, so an attacker who swaps
    // the stored key and re-signs under their own key is caught.
    {
        let (_other_dir, other_path) = tmp_db();
        let other = ArborDb::create(&other_path)
            .unwrap()
            .change_password("pw")
            .unwrap()
            .pubkey()
            .unwrap();
        assert_ne!(other, genuine);

        let guest = ArborDb::open(&path).unwrap().with_pubkey(other).unwrap();
        let t = guest.open_table("data").unwrap();
        assert!(matches!(
            t.read().unwrap().load_value("docs/readme"),
            Err(AdbError::Tampered(_))
        ));
    }

    // Pinning is a guest operation: an authenticated user already verifies through
    // its own key, and an unprotected database has no signatures to check.
    assert!(matches!(
        ArborDb::open_with_authentication(&path, "master", "pw")
            .unwrap()
            .with_pubkey(genuine),
        Err(AdbError::PermissionDenied(_))
    ));
    assert!(matches!(
        ArborDb::create_in_memory().unwrap().with_pubkey(genuine),
        Err(AdbError::NoPermissions)
    ));
}
