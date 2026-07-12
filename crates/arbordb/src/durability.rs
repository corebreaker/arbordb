//! How durable a committed write must be before its
//! [`commit`](crate::txn::WriteTxn::commit) returns.

use redb::Durability as EngineDurability;

/// The durability level of a [`WriteTxn`](crate::txn::WriteTxn) — how much
/// crash-safety its [`commit`](crate::txn::WriteTxn::commit) guarantees, traded
/// against commit latency. Set it with
/// [`WriteTxn::with_durability`](crate::txn::WriteTxn::with_durability); the default
/// is [`Immediate`](Self::Immediate).
///
/// Both levels keep the database **internally consistent** across a crash — a
/// committed transaction never leaves a torn or corrupt state, and an aborted one
/// leaves nothing. They differ only in whether a power loss may roll back the most
/// recent commits. Within a running process a committed write is always visible to
/// later reads whatever the level, and a clean shutdown flushes everything; the
/// distinction matters only for surviving an actual crash.
///
/// Why this is worth exposing: a workload of many small commits is bottlenecked by
/// the per-commit disk flush, not the storage data structure. Batching writes into
/// one transaction, or dropping to [`None`](Self::None), is what recovers that cost.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum Durability {
    /// No flush on commit: the fastest, but a crash may lose an unbounded suffix of
    /// recent commits — they persist only once a later [`Immediate`](Self::Immediate)
    /// commit or a clean shutdown flushes them. For rebuildable, replayable, or
    /// bulk-loaded data where throughput outweighs the last writes' crash-survival.
    None,

    /// The commit is flushed to disk before it returns: full durability, and the
    /// default. A `commit` that returns `Ok` is guaranteed persisted.
    #[default]
    Immediate,
}

impl Durability {
    /// Maps to the storage engine's own durability level. Kept `pub(crate)` so the
    /// engine type never surfaces in the public API.
    pub(crate) fn to_engine(self) -> EngineDurability {
        match self {
            Durability::None => EngineDurability::None,
            Durability::Immediate => EngineDurability::Immediate,
        }
    }
}
