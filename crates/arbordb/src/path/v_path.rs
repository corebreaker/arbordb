//! [`VPath`]: an intra-value navigation path of field names and list indices.

use super::{
    functions::parse_token,
    segment::{Segment, Segments},
    PathTail,
};

use crate::error::{AdbError, AdbResult};
use std::{
    fmt::{Debug, Display, Formatter, Result as FmtResult},
    ops::{Div, DivAssign},
    str::FromStr,
};

/// A parsed value-path identifying a vnode inside a stored [`Value`](crate::Value).
///
/// Object fields are named (`a/b`); list elements are indexed (`a/t[5]`). It is
/// the intra-value navigation path — the counterpart of an
/// [`APath`](super::APath), which addresses a whole value and carries no index.
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct VPath {
    segments: Segments,
}

impl VPath {
    /// The root path (empty), identifying the whole value.
    pub fn root() -> Self {
        Self {
            segments: Segments::new(),
        }
    }

    /// Parses a path string such as `a/b[12]/x`.
    pub fn parse(s: &str) -> AdbResult<Self> {
        s.parse()
    }

    /// Returns `true` if this is the root (empty) path.
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// The number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Returns `true` if there are no segments (the root path).
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The path's segments, in order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The last segment, if any.
    pub fn last(&self) -> Option<&Segment> {
        self.segments.last()
    }

    /// Appends a named segment.
    pub fn push_name(&mut self, name: impl AsRef<str>) {
        match name.as_ref() {
            "." => {}
            ".." => {
                self.segments.pop();
            }
            name => self.segments.push(Segment::Name(name.into())),
        }
    }

    /// Appends an indexed segment.
    pub fn push_index(&mut self, index: u64) {
        self.segments.push(Segment::Index(index));
    }

    /// Returns a copy of this path with a named segment appended.
    pub fn child_name(&self, name: impl AsRef<str>) -> Self {
        let mut path = self.clone();
        path.push_name(name);
        path
    }

    /// Returns a copy of this path with an indexed segment appended.
    pub fn child_index(&self, index: u64) -> Self {
        let mut path = self.clone();
        path.push_index(index);
        path
    }

    /// Appends `tail`'s segments in place.
    pub fn inplace_join(&mut self, tail: &VPath) {
        self.segments.extend(tail.segments.iter().cloned());
    }

    /// Returns this path followed by `tail`'s segments — `tail` resolved relative
    /// to `self`. Joining segment lists (rather than path strings) keeps index
    /// segments unambiguous, since they attach to a name without a separator.
    pub fn join(&self, tail: &VPath) -> Self {
        let mut segments = self.segments.clone();
        segments.extend(tail.segments.iter().cloned());

        VPath {
            segments,
        }
    }

    /// The parent path, or `None` for the root.
    pub fn parent(&self) -> Option<VPath> {
        if self.segments.is_empty() {
            None
        } else {
            Some(VPath {
                segments: self.segments[..self.segments.len() - 1].iter().cloned().collect(),
            })
        }
    }
}

/// `a / b` appends `b` to `a` — a path tail joins segment-wise (`a / b`), a string
/// tail adds one field name (`a / "x"`). Either side may be owned or borrowed; see
/// [`PathTail`].
impl<T: PathTail> Div<T> for VPath {
    type Output = VPath;

    fn div(mut self, rhs: T) -> VPath {
        rhs.append_to(&mut self);
        self
    }
}

impl<T: PathTail> Div<T> for &VPath {
    type Output = VPath;

    fn div(self, rhs: T) -> VPath {
        let mut path = self.clone();
        rhs.append_to(&mut path);
        path
    }
}

impl<T: PathTail> DivAssign<T> for VPath {
    fn div_assign(&mut self, rhs: T) {
        rhs.append_to(self);
    }
}

impl<T: PathTail> DivAssign<T> for &mut VPath {
    fn div_assign(&mut self, rhs: T) {
        rhs.append_to(self);
    }
}

impl FromStr for VPath {
    type Err = AdbError;

    fn from_str(s: &str) -> AdbResult<Self> {
        if s.is_empty() {
            return Ok(VPath::root());
        }

        let mut segments = Segments::new();
        for token in s.split('/') {
            match token {
                "" => return Err(AdbError::InvalidPath(format!("empty segment in '{s}'"))),
                "." => {} // current path — a no-op
                ".." => {
                    // Parent — drop the preceding segment; a `..` past the root is invalid.
                    if segments.pop().is_none() {
                        return Err(AdbError::InvalidPath(format!("'{s}' rises above the root")));
                    }
                }
                _ => parse_token(token, s, &mut segments)?,
            }
        }

        Ok(VPath {
            segments,
        })
    }
}

impl Display for VPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let mut first = true;
        for segment in &self.segments {
            match segment {
                Segment::Name(name) => {
                    if !first {
                        f.write_str("/")?;
                    }
                    f.write_str(name)?;
                }
                Segment::Index(index) => write!(f, "[{index}]")?,
            }

            first = false;
        }

        Ok(())
    }
}

impl Debug for VPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "VPath(\"{self}\")")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(path: &VPath) -> &[Segment] {
        path.segments()
    }

    #[test]
    fn parses_and_displays() {
        let p = VPath::parse("a/t[5]/x").unwrap();

        assert_eq!(
            segs(&p),
            &[
                Segment::Name("a".into()),
                Segment::Name("t".into()),
                Segment::Index(5),
                Segment::Name("x".into()),
            ]
        );

        assert_eq!(p.to_string(), "a/t[5]/x");
    }

    #[test]
    fn root_roundtrips() {
        let p = VPath::parse("").unwrap();

        assert!(p.is_root());
        assert_eq!(p.to_string(), "");
    }

    #[test]
    fn leading_index_and_nested() {
        let p = VPath::parse("[3]/a[0][1]").unwrap();

        assert_eq!(
            segs(&p),
            &[
                Segment::Index(3),
                Segment::Name("a".into()),
                Segment::Index(0),
                Segment::Index(1),
            ]
        );

        assert_eq!(p.to_string(), "[3]/a[0][1]");
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(VPath::parse("a//b").is_err());
        assert!(VPath::parse("/a").is_err());
    }

    #[test]
    fn normalizes_dot_and_dotdot() {
        assert_eq!(VPath::parse("a/./b").unwrap(), VPath::parse("a/b").unwrap());
        assert_eq!(VPath::parse("a/b/../c").unwrap().to_string(), "a/c");
        assert_eq!(VPath::parse("a/x[2]/../y").unwrap().to_string(), "a/x/y");
        assert!(VPath::parse("a/..").unwrap().is_root());
        assert_eq!(VPath::parse(".").unwrap(), VPath::root());
    }

    #[test]
    fn dotdot_above_the_root_is_rejected() {
        assert!(VPath::parse("..").is_err());
        assert!(VPath::parse("a/../..").is_err());
    }

    #[test]
    fn dot_and_dotdot_are_reserved_names() {
        // Only the exact tokens are special; `.foo` stays an ordinary name.
        assert!(VPath::parse("..[0]").is_err());
        assert_eq!(segs(&VPath::parse(".foo").unwrap()), &[Segment::Name(".foo".into())]);
    }

    #[test]
    fn div_operator_joins() {
        let a = VPath::parse("a/b").unwrap();
        let b = VPath::parse("c[0]/d").unwrap();

        assert_eq!((&a / &b).to_string(), "a/b/c[0]/d");
        assert_eq!((a.clone() / b.clone()).to_string(), "a/b/c[0]/d");
        // Joining the root is the identity.
        assert_eq!(a.clone() / VPath::root(), a);
    }

    #[test]
    fn last_and_parent() {
        let p = VPath::parse("a/b[3]").unwrap();

        assert_eq!(p.last(), Some(&Segment::Index(3)));
        assert_eq!(p.parent().unwrap().to_string(), "a/b");
        assert!(VPath::root().last().is_none());
        assert!(VPath::root().parent().is_none());
    }

    #[test]
    fn debug_shows_the_path_string() {
        let p = VPath::parse("a/b[0]").unwrap();

        assert_eq!(format!("{p:?}"), "VPath(\"a/b[0]\")");
    }

    #[test]
    fn div_operator_appends_names_and_paths() {
        let base = VPath::parse("users").unwrap();

        // A string tail is a single field name; chaining builds a path.
        assert_eq!((base.clone() / "alice" / "age").to_string(), "users/alice/age");
        assert_eq!((&base / "alice").to_string(), "users/alice");
        assert_eq!((base.clone() / String::from("bob")).to_string(), "users/bob");

        // A path tail still joins segment-wise, indices included.
        assert_eq!(
            (base.clone() / VPath::parse("t[0]/x").unwrap()).to_string(),
            "users/t[0]/x"
        );

        // `.`/`..` in a name tail normalize like `child_name`.
        assert_eq!((base.clone() / "alice" / "..").to_string(), "users");

        // `/=` appends in place, for names or paths.
        let mut p = VPath::parse("a").unwrap();
        p /= "b";
        p /= VPath::parse("c[1]").unwrap();
        assert_eq!(p.to_string(), "a/b/c[1]");
    }

    #[test]
    fn parse_rejects_reserved_characters_and_stray_indices() {
        // A `]` (or `/`, `[`) inside a name is reserved.
        assert!(VPath::parse("a]b").is_err());
        // Text after a closed index must start a new bracketed index.
        assert!(VPath::parse("a[0]x").is_err());
    }
}
