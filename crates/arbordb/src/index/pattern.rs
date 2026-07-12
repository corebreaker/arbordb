//! Index scope patterns: which entities an index covers, and which a write touched.
//!
//! A pattern is a slash-separated access path in which a `*` segment is a wildcard
//! matching any single child (`users/*` selects every direct child of `users`);
//! every other segment matches literally. It is not recursive — `*` spans exactly
//! one level — and the empty pattern matches the table root (one entity).
//!
//! Given the access path a mutation touched, [`Pattern::affected_entities`]
//! returns exactly the matching entities on that path's root-to-a-node line — the
//! only ones whose indexed columns the mutation could have changed. The walk is
//! pruned to that line, so it visits only nodes the mutation could affect.

use crate::{
    codec::ArchivedDir,
    engine::{entry_split, EntryBytes, EntryKind, read_entry},
    error::{AdbError, AdbResult},
    path::APath,
    AKey,
};

use redb::ReadableTable;

/// A parsed index scope pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pattern {
    /// The parsed segments, in path order.
    segs: Vec<PatternSeg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PatternSeg {
    /// A fixed name that must match exactly.
    Lit(String),
    /// `*`: matches any single child.
    Star,
}

impl Pattern {
    /// Parses a pattern such as `users/*` or `org/teams/*`. The empty string is the
    /// root pattern.
    pub(crate) fn parse(pattern: &str) -> AdbResult<Pattern> {
        let mut segs = Vec::new();
        if pattern.is_empty() {
            return Ok(Pattern {
                segs,
            });
        }

        for token in pattern.split('/') {
            if token.is_empty() {
                return Err(AdbError::InvalidPath(format!(
                    "empty segment in index pattern '{pattern}'"
                )));
            }

            if token == "*" {
                segs.push(PatternSeg::Star);
            } else {
                segs.push(PatternSeg::Lit(token.to_string()));
            }
        }

        Ok(Pattern {
            segs,
        })
    }

    /// The depth (number of access-path levels) at which this pattern's entities
    /// sit. An entity can lie at or under a root only if this is at least the
    /// root's length.
    pub(crate) fn depth(&self) -> usize {
        self.segs.len()
    }

    /// The keys of the entities this pattern matches that lie on the same
    /// root-to-a-node line as `scope` (the path a mutation touched).
    pub(crate) fn affected_entities<R>(&self, data: &R, scope: &APath) -> AdbResult<Vec<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let mut out = Vec::new();
        self.walk(data, 0, AKey::ROOT, scope.names(), 0, &mut out)?;

        Ok(out)
    }

    fn walk<R>(
        &self,
        data: &R,
        si: usize,
        cur: AKey,
        scope: &[smol_str::SmolStr],
        depth: usize,
        out: &mut Vec<AKey>,
    ) -> AdbResult<()>
    where
        R: ReadableTable<u128, EntryBytes>, {
        // Every pattern segment consumed: `cur` is a matched entity.
        if si == self.segs.len() {
            out.push(cur);

            return Ok(());
        }

        match &self.segs[si] {
            PatternSeg::Lit(name) => {
                // Within `scope`, this segment must equal the path the mutation
                // took; otherwise the entity is on a different branch.
                if depth < scope.len() && scope[depth].as_str() != name {
                    return Ok(());
                }

                if let Some(child) = crate::engine::child_of(data, cur, name)? {
                    self.walk(data, si + 1, child, scope, depth + 1, out)?;
                }
            }
            PatternSeg::Star => {
                if depth < scope.len() {
                    // The wildcard binds to the one child the mutation descended into.
                    if let Some(child) = crate::engine::child_of(data, cur, scope[depth].as_str())? {
                        self.walk(data, si + 1, child, scope, depth + 1, out)?;
                    }
                } else {
                    // Past the mutation's path: it sits above these entities, so
                    // every child is in scope.
                    for child in Self::children(data, cur)? {
                        self.walk(data, si + 1, child, scope, depth + 1, out)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// The child keys of directory `akey` (empty if it is absent or not a directory).
    fn children<R>(data: &R, akey: AKey) -> AdbResult<Vec<AKey>>
    where
        R: ReadableTable<u128, EntryBytes>, {
        let Some(entry) = read_entry(data, akey)? else {
            return Ok(Vec::new());
        };

        let (kind, payload) = entry_split(&entry)?;
        if kind != EntryKind::Dir {
            return Ok(Vec::new());
        }

        Ok(ArchivedDir::new(payload)?
            .entries()?
            .into_iter()
            .map(|(_, child)| child)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(name: &str) -> PatternSeg {
        PatternSeg::Lit(name.to_string())
    }

    #[test]
    fn parses_a_wildcard_pattern() {
        assert_eq!(
            Pattern::parse("users/*").unwrap().segs,
            vec![lit("users"), PatternSeg::Star]
        );
    }

    #[test]
    fn the_empty_pattern_matches_the_root() {
        assert!(Pattern::parse("").unwrap().segs.is_empty());
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(Pattern::parse("users//*").is_err());
        assert!(Pattern::parse("/users").is_err());
    }
}
