//! [`IndexEntry`] — one row of the registry: an [`IndexDef`] bound to its owning
//! table and the [`IndexId`] allocated to it.

use super::super::IndexId;
use crate::index::definitions::IndexDef;

/// One registered index: its owning table, allocated id, and definition.
pub(crate) struct IndexEntry {
    /// The table this index belongs to.
    table: String,
    /// The compact id allocated to this index.
    id:    IndexId,
    /// The index definition.
    def:   IndexDef,
}

impl IndexEntry {
    /// Binds `def` to its owning `table` and allocated `id`.
    pub(super) fn new(table: String, id: IndexId, def: IndexDef) -> Self {
        Self {
            table,
            id,
            def,
        }
    }

    /// The table this index belongs to.
    pub(crate) fn table(&self) -> &str {
        &self.table
    }

    /// The compact id allocated to this index.
    pub(crate) fn id(&self) -> IndexId {
        self.id
    }

    /// The index definition.
    pub(crate) fn def(&self) -> &IndexDef {
        &self.def
    }

    /// Consumes the entry, yielding its definition.
    pub(crate) fn into_def(self) -> IndexDef {
        self.def
    }
}
