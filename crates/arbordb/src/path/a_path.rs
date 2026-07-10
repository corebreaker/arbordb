//! [`APath`]: a slash-separated, name-only address of a whole stored value.

use super::functions::validate_name;
use crate::error::{AdbError, AdbResult};
use smallvec::SmallVec;
use smol_str::SmolStr;
use std::{
    fmt::{Debug, Display, Formatter, Result as FmtResult},
    str::FromStr,
};

/// The inline capacity of an access path's name buffer. Access paths are short
/// (a value's address in the virtual filesystem), so a small inline buffer keeps
/// the common case off the heap.
const INLINE_NAMES: usize = 3;

/// An access path's name storage: inline up to [`INLINE_NAMES`], heap beyond.
type Names = SmallVec<[SmolStr; INLINE_NAMES]>;

/// A filesystem-like address of a stored [`Value`](crate::Value).
///
/// An [`APath`] is a slash-separated sequence of **names only** — unlike a
/// [`VPath`](super::VPath), it carries no list index `[i]`, since it addresses a
/// whole value ("a file"), never a scalar inside one. It is encoded
/// order-preservingly straight into the storage key, so a common prefix groups
/// related values (listing a "directory" is a prefix scan). Parsing normalizes
/// `.`/`..` the same way `VPath` does.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct APath {
    /// The path's names, from root to leaf (empty for the root path).
    names: Names,
}

impl APath {
    /// The root path (empty).
    pub fn root() -> Self {
        Self {
            names: Names::new()
        }
    }

    /// Parses a path string such as `users/alice`.
    pub fn parse(s: &str) -> AdbResult<Self> {
        s.parse()
    }

    /// Returns `true` if this is the root (empty) path.
    pub fn is_root(&self) -> bool {
        self.names.is_empty()
    }

    /// The number of names.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Returns `true` if there are no names (the root path).
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The path's names, in order.
    pub fn names(&self) -> &[SmolStr] {
        &self.names
    }

    /// The last name, if any.
    pub fn last(&self) -> Option<&str> {
        self.names.last().map(SmolStr::as_str)
    }

    /// Appends a name (`.` is a no-op, `..` drops the preceding name).
    pub fn push_name(&mut self, name: impl AsRef<str>) {
        match name.as_ref() {
            "." => {}
            ".." => {
                self.names.pop();
            }
            name => self.names.push(name.into()),
        }
    }

    /// Returns a copy of this path with a name appended.
    pub fn child_name(&self, name: impl AsRef<str>) -> Self {
        let mut path = self.clone();
        path.push_name(name);
        path
    }

    /// Returns this path followed by `tail`'s names.
    pub fn join(&self, tail: &APath) -> Self {
        let mut names = self.names.clone();
        names.extend(tail.names.iter().cloned());

        APath {
            names,
        }
    }

    /// The parent path, or `None` for the root.
    pub fn parent(&self) -> Option<APath> {
        if self.names.is_empty() {
            None
        } else {
            Some(APath {
                names: self.names[..self.names.len() - 1].iter().cloned().collect(),
            })
        }
    }

    /// Splits into the parent path and the last name, or `None` for the root.
    pub(crate) fn split_last(&self) -> Option<(APath, &str)> {
        self.names.split_last().map(|(last, head)| {
            (
                APath {
                    names: head.iter().cloned().collect(),
                },
                last.as_str(),
            )
        })
    }
}

impl FromStr for APath {
    type Err = AdbError;

    fn from_str(s: &str) -> AdbResult<Self> {
        if s.is_empty() {
            return Ok(APath::root());
        }

        let mut names = Names::new();
        for token in s.split('/') {
            match token {
                "" => return Err(AdbError::InvalidPath(format!("empty segment in '{s}'"))),
                "." => {} // current path — a no-op
                ".." => {
                    if names.pop().is_none() {
                        return Err(AdbError::InvalidPath(format!("'{s}' rises above the root")));
                    }
                }
                name => {
                    // An access path has no list index; `validate_name` rejects `[`/`]`.
                    validate_name(name, s)?;
                    names.push(name.into());
                }
            }
        }

        Ok(APath {
            names,
        })
    }
}

impl Display for APath {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let mut first = true;
        for name in &self.names {
            if !first {
                f.write_str("/")?;
            }
            f.write_str(name)?;

            first = false;
        }

        Ok(())
    }
}

impl Debug for APath {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "APath(\"{self}\")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays() {
        let p = APath::parse("users/alice").unwrap();

        assert_eq!(p.names(), &[SmolStr::new("users"), SmolStr::new("alice")]);
        assert_eq!(p.to_string(), "users/alice");
        assert_eq!(p.last(), Some("alice"));
    }

    #[test]
    fn root_roundtrips() {
        let p = APath::parse("").unwrap();

        assert!(p.is_root());
        assert!(p.is_empty());
        assert_eq!(p.to_string(), "");
        assert!(p.last().is_none());
        assert!(p.parent().is_none());
    }

    #[test]
    fn rejects_list_index_and_empty_segments() {
        assert!(APath::parse("users/alice[0]").is_err());
        assert!(APath::parse("a//b").is_err());
        assert!(APath::parse("/a").is_err());
    }

    #[test]
    fn normalizes_dot_and_dotdot() {
        assert_eq!(APath::parse("a/./b").unwrap(), APath::parse("a/b").unwrap());
        assert_eq!(APath::parse("a/b/../c").unwrap().to_string(), "a/c");
        assert!(APath::parse("a/..").unwrap().is_root());
        assert!(APath::parse("..").is_err());
    }

    #[test]
    fn child_join_and_parent() {
        let base = APath::parse("users").unwrap();

        assert_eq!(base.child_name("alice").to_string(), "users/alice");
        assert_eq!(
            base.join(&APath::parse("bob/pet").unwrap()).to_string(),
            "users/bob/pet"
        );
        assert_eq!(base.child_name("alice").parent().unwrap(), base);
    }

    #[test]
    fn debug_shows_the_path_string() {
        let p = APath::parse("a/b").unwrap();

        assert_eq!(format!("{p:?}"), "APath(\"a/b\")");
    }
}
