//! Coverage for the `permissions` admin surface: promotion back-fill of pre-existing
//! data, the user/group lifecycle, and the guards that reject built-in and frozen
//! principals or a non-administrator.

#![cfg(feature = "permissions")]

use arbordb::{
    acl::{AclClass, Rights},
    data::Scalar,
    AdbError,
    ArborDb,
    Value,
};

use std::path::PathBuf;

fn leaf(n: i64) -> Value {
    Value::Leaf(Scalar::I64(n))
}

fn tmp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.redb");

    (dir, path)
}

#[test]
fn promotion_backfills_acls_and_integrity_on_preexisting_data() {
    let db = ArborDb::create_in_memory().unwrap();

    // Write data while the database is still unprotected.
    {
        let table = db.open_table("t").unwrap();
        let w = table.write().unwrap();
        w.store_value("a/b", &leaf(1)).unwrap();
        w.store_value("c", &leaf(2)).unwrap();
        w.commit().unwrap();
    }

    // Promotion stamps a master-owned ACL and seals integrity on every pre-existing
    // a-node, so the data stays readable and is now owned.
    let master = db.change_password("pw").unwrap();
    let table = master.open_table("t").unwrap();
    let r = table.read().unwrap();

    assert_eq!(r.load_value("a/b").unwrap(), Some(leaf(1)));
    assert_eq!(r.load_value("c").unwrap(), Some(leaf(2)));
    assert_eq!(r.owner("a/b").unwrap(), Some(String::from("master")));
}

#[test]
fn user_and_group_lifecycle_round_trips() {
    let master = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();

    // `create_group` also makes a same-named group and adds the user to it.
    master.add_user("bob", "pw1", true).unwrap();
    master.add_user("alice", "pw2", false).unwrap();
    master.add_group("staff").unwrap();

    master.assign_user_to_group("alice", "staff").unwrap();
    master.assign_user_to_group("alice", "staff").unwrap(); // idempotent
    master.remove_user_from_group("alice", "staff").unwrap();
    master.remove_user_from_group("alice", "staff").unwrap(); // idempotent

    master.rename_user("alice", "alicia").unwrap();
    master.rename_group("staff", "team").unwrap();

    assert!(master.list_users().unwrap().contains(&String::from("alicia")));
    assert!(master.list_groups().unwrap().contains(&String::from("team")));

    master.remove_user("alicia").unwrap();
    assert!(!master.list_users().unwrap().contains(&String::from("alicia")));
}

#[test]
fn admin_guards_reject_built_ins_frozen_users_and_clashes() {
    let master = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
    master.add_group("staff").unwrap();

    // The built-in master user and the built-in groups cannot be renamed.
    assert!(matches!(
        master.rename_user("master", "boss"),
        Err(AdbError::PermissionDenied(_))
    ));
    assert!(matches!(
        master.rename_group("super", "elite"),
        Err(AdbError::PermissionDenied(_))
    ));

    // Renaming a group onto an existing name clashes.
    master.add_group("other").unwrap();
    assert!(master.rename_group("staff", "other").is_err());

    // The frozen guest user cannot be assigned to any group.
    assert!(matches!(
        master.assign_user_to_group("guest", "other"),
        Err(AdbError::Frozen(_))
    ));

    // Unknown users / groups are reported, not silently ignored.
    assert!(master.assign_user_to_group("nobody", "other").is_err());
    assert!(master.assign_user_to_group("guest", "nogroup").is_err());
    assert!(master.rename_user("nobody", "x").is_err());
    assert!(master.rename_group("nogroup", "x").is_err());
}

#[test]
fn a_guest_cannot_perform_administration() {
    let (_dir, path) = tmp_db();
    ArborDb::create(&path).unwrap().change_password("pw").unwrap();

    // Reopening without credentials lands on the read-only guest principal.
    let guest = ArborDb::open(&path).unwrap();
    assert_eq!(guest.current_user(), Some("guest"));

    // Every administration entry point is refused — both the `admin_key` and the
    // `admin_creds` guards.
    assert!(matches!(guest.add_group("g"), Err(AdbError::PermissionDenied(_))));
    assert!(matches!(
        guest.add_user("u", "p", false),
        Err(AdbError::PermissionDenied(_))
    ));
    assert!(matches!(
        guest.rename_user("master", "x"),
        Err(AdbError::PermissionDenied(_))
    ));
    assert!(matches!(
        guest.assign_user_to_group("master", "super"),
        Err(AdbError::PermissionDenied(_))
    ));
}

#[test]
fn acl_queries_handle_absent_paths_the_root_and_unknown_groups() {
    let master = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
    let table = master.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.store_value("file", &leaf(1)).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // An absent path: `get_acl` errors, `owner`/`groups` report nothing.
    assert!(r.get_acl("missing", AclClass::User).is_err());
    assert_eq!(r.owner("missing").unwrap(), None);
    assert!(r.groups("missing").unwrap().is_empty());

    // The root carries no stored ACL: no rights, no owner, no groups.
    assert_eq!(r.get_acl("", AclClass::User).unwrap(), Rights::None);
    assert_eq!(r.owner("").unwrap(), None);
    assert!(r.groups("").unwrap().is_empty());

    // A group class naming a group that does not exist grants nothing.
    assert_eq!(
        r.get_acl("file", AclClass::Group(String::from("nogroup"))).unwrap(),
        Rights::None
    );
}

#[test]
fn acl_name_resolution_through_a_write_transaction() {
    let master = ArborDb::create_in_memory().unwrap().change_password("pw").unwrap();
    master.add_group("staff").unwrap();
    let table = master.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store_value("file", &leaf(1)).unwrap();
    w.add_group("file", "staff").unwrap();

    // Reading ACL metadata through the writer resolves the owner and group *names*
    // via the write transaction's own name-resolution path.
    assert_eq!(w.owner("file").unwrap(), Some(String::from("master")));
    assert_eq!(w.groups("file").unwrap(), vec![String::from("staff")]);
    assert_eq!(
        w.get_acl("file", AclClass::Group(String::from("staff"))).unwrap(),
        Rights::Access
    );

    w.commit().unwrap();
}

#[test]
fn chown_is_allowed_for_the_owner_and_refused_for_a_non_owner() {
    let (_dir, path) = tmp_db();

    {
        let db = ArborDb::create(&path).unwrap().change_password("m").unwrap();
        db.add_user("alice", "a", false).unwrap();
        db.add_user("bob", "b", false).unwrap();
    }

    // Alice creates and thus owns a file, and — as its owner — may hand it to Bob.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        w.store_value("f", &leaf(1)).unwrap();
        w.chown("f", "bob").unwrap();
        w.commit().unwrap();
    }

    // Alice no longer owns it (and is no administrator), so she may not chown it back.
    {
        let alice = ArborDb::open_with_authentication(&path, "alice", "a").unwrap();
        let t = alice.open_table("data").unwrap();
        let w = t.write().unwrap();
        assert!(matches!(w.chown("f", "alice"), Err(AdbError::PermissionDenied(_))));
    }
}
