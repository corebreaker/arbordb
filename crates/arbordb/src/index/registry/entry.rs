use super::super::IndexId;
use crate::index::definitions::IndexDef;

/// One registered index: its owning table, allocated id, and definition.
pub(crate) struct IndexEntry {
    table: String,
    id:    IndexId,
    def:   IndexDef,
}

impl IndexEntry {
    pub(super) fn new(table: String, id: IndexId, def: IndexDef) -> Self {
        Self {
            table,
            id,
            def,
        }
    }

    pub(crate) fn table(&self) -> &str {
        &self.table
    }

    pub(crate) fn id(&self) -> IndexId {
        self.id
    }

    pub(crate) fn def(&self) -> &IndexDef {
        &self.def
    }

    pub(crate) fn into_def(self) -> IndexDef {
        self.def
    }
}
