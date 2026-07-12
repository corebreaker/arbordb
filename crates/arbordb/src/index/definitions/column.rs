//! [`IndexColumn`] — one column of an index: an intra-value [`VPath`] plus a sort
//! [`Direction`].

use super::Direction;
use crate::path::VPath;

/// One column of an index: the value found at `path` (an intra-value [`VPath`]
/// relative to each indexed entity), sorted in `direction`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IndexColumn {
    /// Path of the column value, relative to a matched entity.
    path:      VPath,
    /// Sort direction for this column.
    direction: Direction,
}

impl IndexColumn {
    /// A column over `path` with an explicit sort `direction`.
    pub(super) fn new(path: VPath, direction: Direction) -> Self {
        Self {
            path,
            direction,
        }
    }

    /// An ascending column over `path`.
    pub fn asc(path: VPath) -> Self {
        Self {
            path,
            direction: Direction::Asc,
        }
    }

    /// A descending column over `path`.
    pub fn desc(path: VPath) -> Self {
        Self {
            path,
            direction: Direction::Desc,
        }
    }

    /// The column value's path, relative to a matched entity.
    pub fn path(&self) -> &VPath {
        &self.path
    }

    /// This column's sort direction.
    pub fn direction(&self) -> Direction {
        self.direction
    }
}
