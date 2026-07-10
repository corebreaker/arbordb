#![cfg(feature = "permissions")]

use arbordb::{
    ArborDb,
    AdbResult,
    AdbError,
    Value,
    data::Scalar,
    acl::{Mode, Rights},
};
use redb::{Database, ReadableTable, TableDefinition, TableHandle};
use std::{error::Error, path::Path};

const PASSWORD_MASTER: &str = "Master-Pass1";
const PASSWORD_USER1: &str = "User1:Pass2";
const PASSWORD_USER2: &str = "User2:Pass3";

const USERNAME1: &str = "one-user";
const USERNAME2: &str = "another-user";

const GROUPNAME: &str = "the-group";

const TABLE_NAME: &str = "the-table";

const PARENT1: &str = "a";
const PATH1: &str = "a/v1";
const PATH2: &str = "a/v2";

const PARENT2: &str = "b";
const PATH3: &str = "b/x";
const PATH4: &str = "b/z";
const PATH5: &str = "a/v3";

const VALUE: &str = "hello";

pub(crate) const TABLE_META: TableDefinition<&str, &[u8]> = TableDefinition::new("$metadata");
pub(crate) const TABLE_USER: TableDefinition<u128, &'static [u8]> = TableDefinition::new(TABLE_NAME);

fn create_db(file: &Path) -> AdbResult<()> {
    let db = ArborDb::create(file)?;
    assert!(!db.is_protected()?, "Database should not be protected");
    let db = db.change_password(PASSWORD_MASTER)?;
    assert!(db.is_protected()?, "Database should be protected");

    let mut users = vec![String::from("guest"), String::from("master")];
    assert_eq!(db.list_users()?, users, "Bad list of users before user creation",);

    db.add_user(USERNAME1, PASSWORD_USER1, false)?;
    db.add_user(USERNAME2, PASSWORD_USER2, false)?;

    users.extend_from_slice(&[String::from(USERNAME1), String::from(USERNAME2)]);
    users.sort();
    assert_eq!(db.list_users()?, users, "Bad list of users after user creation",);

    db.add_group(GROUPNAME)?;
    db.assign_user_to_group(USERNAME1, GROUPNAME)?;
    db.assign_user_to_group(USERNAME2, GROUPNAME)?;

    let mut groups = vec![String::from(USERNAME1), String::from(USERNAME2)];
    groups.sort();
    assert_eq!(
        db.group_members(GROUPNAME)?,
        groups,
        "Bad list of member for group {GROUPNAME}"
    );

    let db = db.with_authentication(USERNAME1, PASSWORD_USER1)?;
    let table = db.open_table(TABLE_NAME)?;
    {
        let txn = table.write()?;
        let v1 = Value::Leaf(Scalar::I32(1234));
        let v2 = Value::Leaf(Scalar::Str(String::from(VALUE)));
        let v3 = Value::Leaf(Scalar::Str(String::from("foobar")));

        txn.store_value(PATH1, &v1)?;
        txn.store_value(PATH2, &v2)?;
        txn.store_value(PATH3, &v3)?;

        txn.chgrp(PARENT1, Some(GROUPNAME))?;
        txn.chgrp(PATH1, Some(GROUPNAME))?;
        txn.chgrp(PATH2, Some(GROUPNAME))?;
        txn.chgrp(PATH3, Some(GROUPNAME))?;

        txn.chmod(
            PARENT1,
            Mode {
                owner: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                group: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                other: Rights {
                    read:  true,
                    write: false,
                    walk:  true,
                },
            },
        )?;

        txn.chmod(
            PARENT2,
            Mode {
                owner: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                group: Rights::read_only(),
                other: Rights::default(),
            },
        )?;

        txn.chmod(
            PATH1,
            Mode {
                owner: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                group: Rights::read_only(),
                other: Rights::default(),
            },
        )?;

        txn.chmod(
            PATH2,
            Mode {
                owner: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                group: Rights::read_only(),
                other: Rights::read_only(),
            },
        )?;

        txn.chmod(
            PATH3,
            Mode {
                owner: Rights {
                    read:  true,
                    write: true,
                    walk:  true,
                },
                group: Rights::read_only(),
                other: Rights::default(),
            },
        )?;

        txn.chown(PATH2, USERNAME2)?;
        txn.commit()?;
    }

    Ok(())
}

fn open_db_before_alteration(file: &Path) -> AdbResult<()> {
    let db = ArborDb::open(file)?;
    assert!(db.is_protected()?, "Database should be protected");
    let db = db.with_authentication(USERNAME2, PASSWORD_USER2)?;
    let table = db.open_table(TABLE_NAME)?;
    let txn = table.read()?;

    let v1 = txn.load_value(PATH1)?;
    let v2 = txn.load_value(PATH2)?;
    let v3 = txn.load_value(PATH3);
    let v4 = txn.load_value(PATH4);
    let v5 = txn.load_value(PATH5)?;

    assert_eq!(v1, Some(Value::Leaf(Scalar::I32(1234))));
    assert_eq!(v2, Some(Value::Leaf(Scalar::Str(String::from(VALUE)))));
    assert!(
        matches!(v3, Err(AdbError::PermissionDenied(_))),
        "Value 3 should be in error: {v3:?}"
    );

    assert!(
        matches!(v4, Err(AdbError::PermissionDenied(_))),
        "Value 4 should be in error: {v4:?}"
    );

    assert_eq!(v5, None::<Value>);

    Ok(())
}

fn open_db_after_alteration_1(file: &Path) -> AdbResult<()> {
    let db = ArborDb::open(file)?;
    assert!(db.is_protected()?, "Database should be protected");

    let db = db.with_authentication(USERNAME2, PASSWORD_USER2)?;
    assert!(db.is_protected()?, "Database should be protected");

    Ok(())
}

fn open_db_after_alteration_2(file: &Path) -> AdbResult<()> {
    let db = ArborDb::open(file)?;
    assert!(db.is_protected()?, "Database should be protected");

    let table = db.open_table(TABLE_NAME)?;
    let txn = table.read()?;
    let v = txn.load_value(PATH2)?;
    println!("Got value: {v:?}");

    Ok(())
}

fn open_db_after_alteration_2_with_authentication(file: &Path) -> AdbResult<()> {
    let db = ArborDb::open_with_authentication(file, USERNAME2, PASSWORD_USER2)?;

    let table = db.open_table(TABLE_NAME)?;
    let txn = table.read()?;
    let v = txn.load_value(PATH2)?;
    println!("Got value: {v:?}");

    Ok(())
}

fn alter1_db(file: &Path) -> Result<(), Box<dyn Error>> {
    let db = Database::open(file)?;
    let txn = db.begin_write()?;

    {
        let tables = txn.list_tables()?.map(|t| t.name().to_string()).collect::<Vec<_>>();
        println!("Tables: {tables:?}");

        let mut table = txn.open_table(TABLE_META)?;
        if let Some(mut data) = table.get_mut("groups")? {
            let mut bytes = data.value().to_vec();
            let idx = bytes.len() - 1;

            bytes[idx] = 1;
            data.insert(bytes.as_slice())?;
        }
    }

    txn.commit()?;
    Ok(())
}

fn alter2_db(file: &Path) -> Result<(), Box<dyn Error>> {
    let db = Database::open(file)?;
    let txn = db.begin_write()?;

    {
        let mut table = txn.open_table(TABLE_USER)?;

        let mut found_key = None::<u128>;
        for item in table.iter()? {
            let (name, value) = item?;
            let key = name.value();
            let value = value.value();

            if value.ends_with(VALUE.as_bytes()) {
                found_key.replace(key);
                break;
            }
        }

        let Some(key) = found_key else {
            panic!("Value not found");
        };

        if let Some(mut data) = table.get_mut(key)? {
            let mut bytes = data.value().to_vec();
            let idx = bytes.len() - 1;

            bytes[idx] = b'x';
            data.insert(bytes.as_slice())?;
        }
    }

    txn.commit()?;
    Ok(())
}

fn create_and_check_db(file: &Path) {
    if let Err(err) = create_db(file) {
        panic!("Error while creating DB {file}: {err}", file = file.display());
    }

    if let Err(err) = open_db_before_alteration(file) {
        panic!(
            "Error while opening DB {file} before alteration: {err}",
            file = file.display()
        );
    }
}

#[test]
fn integrity() {
    let dir = tempfile::tempdir().unwrap();

    let file1 = dir.path().join("sample-1.db");
    create_and_check_db(&file1);

    let file2 = dir.path().join("sample-2.db");
    create_and_check_db(&file2);

    let file3 = dir.path().join("sample-3.db");
    create_and_check_db(&file3);

    if let Err(err) = alter1_db(&file1) {
        panic!(
            "Error while doing alteration 1 on DB {file}: {err}",
            file = file1.display()
        );
    }

    if let Err(err) = alter2_db(&file2) {
        panic!(
            "Error while doing alteration 2 on DB {file}: {err}",
            file = file2.display()
        );
    }

    if let Err(err) = alter2_db(&file3) {
        panic!(
            "Error while doing alteration 2 on DB {file}: {err}",
            file = file3.display()
        );
    }

    let res = open_db_after_alteration_1(&file1);
    assert!(
        matches!(res, Err(AdbError::Tampered(_))),
        "DB {file} should have an error after alteration 1: {res:?}",
        file = file1.display(),
    );

    if let Err(err) = open_db_after_alteration_1(&file2) {
        println!(
            "Error while opening DB {file} for first checking alteration 1: {err}",
            file = file2.display()
        );
    }

    let res = open_db_after_alteration_2_with_authentication(&file2);
    assert!(
        matches!(res, Err(AdbError::Tampered(_))),
        "DB {file} should have an error after alteration 2 with authentication: {res:?}",
        file = file2.display(),
    );

    if let Err(err) = open_db_after_alteration_1(&file3) {
        println!(
            "Error while opening DB {file} for second checking alteration 1: {err}",
            file = file3.display()
        );
    }

    let res = open_db_after_alteration_2(&file3);
    assert!(
        matches!(res, Err(AdbError::Tampered(_))),
        "DB {file} should have an error after alteration 2: {res:?}",
        file = file3.display(),
    );
}
