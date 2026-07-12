//! Per-a-node created / modified / accessed timestamps with the `entry-timestamps`
//! feature.
//!
//! Timestamps live out-of-band in a reserved side table, so a binary built without
//! this feature opens the same database and simply ignores them. Access times are
//! buffered in memory and made durable on the next committed write or an explicit
//! flush, so a read burst does not turn into a write burst.
//!
//! Run with: `cargo run --example timestamps --features entry-timestamps`

use arbordb::{data::Scalar, AdbResult, ArborDb, Value};

fn leaf(n: i64) -> Value {
    Value::Leaf(Scalar::I64(n))
}

fn main() -> AdbResult<()> {
    let db = ArborDb::create_in_memory()?;
    let table = db.open_table("docs")?;

    // Creating a file stamps all three times.
    {
        let w = table.write()?;
        w.store_value("note", &leaf(1))?;
        w.commit()?;
    }

    let created = {
        let r = table.read()?;
        let times = r.times("note")?.expect("a timestamped a-node");
        println!("created  = {}", times.created());
        println!("modified = {}", times.modified());

        times.created()
    };

    // Overwriting the file advances `modified` while `created` stays put.
    {
        let w = table.write()?;
        w.store_value("note", &leaf(2))?;
        w.commit()?;
    }
    {
        let r = table.read()?;
        let times = r.times("note")?.unwrap();
        println!("created unchanged after overwrite = {}", times.created() == created);
    }

    // Reads buffer the access time in memory; it becomes durable on the next
    // committed write or an explicit flush.
    {
        let r = table.read()?;
        let _ = r.load_value("note")?;
    }
    db.flush_access_times()?;
    {
        let r = table.read()?;
        println!("accessed = {}", r.times("note")?.unwrap().accessed());
    }

    Ok(())
}
