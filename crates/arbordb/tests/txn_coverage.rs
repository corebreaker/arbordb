//! Coverage for transaction read-path branches: file-vs-directory mismatches,
//! absent paths, and a root-scoped query below the index pattern's depth.

use arbordb::{data::Leaf, entry::EntryKind, ArborDb};

#[test]
fn read_ops_distinguish_files_directories_and_absent_paths() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.mkdir("dir").unwrap();
        w.store::<i64>("file", &5).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // A whole-value read of a directory errors — a directory has no value of its own.
    assert!(r.load::<i64>("dir").is_err());
    assert!(r.load_value("dir").is_err());
    assert!(r.fetch::<Leaf<'static, i64>>("dir").is_err());

    #[cfg(feature = "serde")]
    assert!(r.load_serde_value::<i64>("dir").is_err());

    // `get` / `get_as` into a directory yield nothing rather than erroring.
    assert_eq!(r.get("dir", "x").unwrap(), None);
    assert_eq!(r.get_as::<i64>("dir", "x").unwrap(), None);

    // `ls` lists a directory, errors on a file, and errors on an absent path.
    assert_eq!(r.ls("dir").unwrap().len(), 0);
    assert!(r.ls("file").is_err());
    assert!(r.ls("missing").is_err());

    // A brand-new table's root directory is not materialized yet: listing it is empty.
    let fresh = db.open_table("fresh").unwrap();
    assert_eq!(fresh.read().unwrap().ls("").unwrap().len(), 0);
}

#[test]
fn storing_a_file_over_a_directory_replaces_it_and_cascades() {
    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();

    {
        let w = table.write().unwrap();
        w.mkdir("d/sub").unwrap();
        // Storing a file where a directory sits replaces it, dropping the subtree.
        w.store::<i64>("d", &1).unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();
    assert_eq!(r.kind("d").unwrap(), Some(EntryKind::File));
    assert_eq!(r.load::<i64>("d").unwrap(), Some(1));
    assert!(!r.exists("d/sub").unwrap());
}

#[cfg(feature = "derive")]
#[test]
fn a_rooted_query_below_the_index_pattern_depth_is_empty() {
    use arbordb::AData;

    #[derive(AData, Debug, Clone, PartialEq)]
    #[arbor(index(name = "by_age", columns(age)))]
    struct Profile {
        age: i64,
    }

    let db = ArborDb::create_in_memory().unwrap();
    let table = db.open_table("t").unwrap();
    table.create_indexes::<Profile>("members/*").unwrap();

    {
        let w = table.write().unwrap();
        w.store::<Profile>(
            "members/alice",
            &Profile {
                age: 30
            },
        )
        .unwrap();
        w.commit().unwrap();
    }

    let r = table.read().unwrap();

    // The index pattern `members/*` is two segments deep; a view rooted three deep sits
    // below it, so the scan is scoped out entirely.
    let deep = r.rooted("members/alice/profile").unwrap();
    assert!(deep.find::<Profile>("by_age", &[]).unwrap().is_empty());
}
