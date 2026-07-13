//! Write-path integrity: a mutation that reads an existing value or descends through
//! a directory re-verifies that a-node's keyed MAC first, so a blob altered outside the
//! library (an attacker with only redb, no integrity key) is caught with
//! [`AdbError::Tampered`] before the write proceeds.

#![cfg(feature = "permissions")]

use arbordb::{data::Scalar, AdbError, ArborDb, Value};

use redb::{Database, ReadableTable, TableDefinition};
use std::path::{Path, PathBuf};

/// The data table backing the `"t"` table — an `AKey` (`u128`) to an entry blob.
const DATA: TableDefinition<u128, &'static [u8]> = TableDefinition::new("t");

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.redb");

    (dir, path)
}

/// Creates a protected database with one nested file, using distinctive names so the
/// raw tamper can find the directory blob and the file blob unambiguously.
fn seed(path: &Path) {
    let db = ArborDb::create(path).unwrap().change_password("pw").unwrap();
    let table = db.open_table("t").unwrap();

    let w = table.write().unwrap();
    w.store_value(
        "dir_alpha/file_beta",
        &Value::Leaf(Scalar::Str(String::from("payload_gamma"))),
    )
    .unwrap();
    w.commit().unwrap();
}

/// Flips the last byte of the first data-table entry whose blob contains `needle` —
/// exactly the reach an attacker holding only redb (and not the integrity key) has.
fn tamper_entry_containing(path: &Path, needle: &[u8]) {
    let db = Database::open(path).unwrap();
    let txn = db.begin_write().unwrap();
    {
        let mut table = txn.open_table(DATA).unwrap();

        let mut found = None;
        for item in table.iter().unwrap() {
            let (key, value) = item.unwrap();
            if value.value().windows(needle.len()).any(|window| window == needle) {
                found = Some(key.value());
                break;
            }
        }

        let key = found.expect("no data entry contained the needle");
        if let Some(mut data) = table.get_mut(key).unwrap() {
            let mut bytes = data.value().to_vec();
            let last = bytes.len() - 1;
            bytes[last] ^= 0x01;
            data.insert(bytes.as_slice()).unwrap();
        }
    }
    txn.commit().unwrap();
}

#[test]
fn a_write_reading_a_tampered_file_value_is_rejected() {
    let (_dir, path) = tmp();
    seed(&path);

    // Alter the file's own value blob.
    tamper_entry_containing(&path, b"payload_gamma");

    let db = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
    let table = db.open_table("t").unwrap();
    let w = table.write().unwrap();

    // Copying the file reads its (now-altered) value blob and re-verifies it first.
    assert!(matches!(
        w.cp("dir_alpha/file_beta", "dir_alpha/copy"),
        Err(AdbError::Tampered(_))
    ));
}

#[test]
fn a_write_descending_through_a_tampered_directory_is_rejected() {
    let (_dir, path) = tmp();
    seed(&path);

    // Alter the directory blob (it holds the child name "file_beta").
    tamper_entry_containing(&path, b"file_beta");

    let db = ArborDb::open_with_authentication(&path, "master", "pw").unwrap();
    let table = db.open_table("t").unwrap();
    let w = table.write().unwrap();

    // A read on the write transaction runs the enforced directory walk, verifying each
    // directory it descends through — the altered blob's MAC no longer matches.
    assert!(matches!(
        w.load_value("dir_alpha/file_beta"),
        Err(AdbError::Tampered(_))
    ));

    // Storing a new file under the directory descends through it via the write context,
    // which likewise re-verifies the directory first.
    assert!(matches!(
        w.store_value("dir_alpha/new_file", &Value::Leaf(Scalar::I32(1))),
        Err(AdbError::Tampered(_))
    ));
}
