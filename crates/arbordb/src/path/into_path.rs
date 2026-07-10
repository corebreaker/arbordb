//! Conversion into a [`VPath`] for path-addressed APIs.

use super::VPath;
use crate::error::AdbResult;

/// A value accepted where an intra-value path is expected: an owned or borrowed
/// [`VPath`] (used as-is) or a string (parsed, with the same normalization as
/// [`VPath::parse`]).
///
/// Path-addressed methods take `impl IntoPath`, so a literal like `"a/b[2]"` and
/// an already-built [`VPath`] are both accepted without a separate overload.
/// Parsing is fallible — a malformed string surfaces as an
/// [`InvalidPath`](crate::AdbError::InvalidPath) error — while a [`VPath`] passes
/// through unparsed.
pub trait IntoPath {
    /// Converts `self` into a [`VPath`], parsing it if it is a string.
    fn into_path(self) -> AdbResult<VPath>;
}

impl IntoPath for VPath {
    fn into_path(self) -> AdbResult<VPath> {
        Ok(self)
    }
}

impl IntoPath for &VPath {
    fn into_path(self) -> AdbResult<VPath> {
        Ok(self.clone())
    }
}

impl IntoPath for &str {
    fn into_path(self) -> AdbResult<VPath> {
        self.parse()
    }
}

impl IntoPath for String {
    fn into_path(self) -> AdbResult<VPath> {
        self.parse()
    }
}

impl IntoPath for &String {
    fn into_path(self) -> AdbResult<VPath> {
        self.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_forms_parse() {
        let want = VPath::parse("a/b[2]/c").unwrap();

        assert_eq!("a/b[2]/c".into_path().unwrap(), want);
        assert_eq!(String::from("a/b[2]/c").into_path().unwrap(), want);
        assert_eq!((&String::from("a/b[2]/c")).into_path().unwrap(), want);
    }

    #[test]
    fn vpath_forms_pass_through() {
        let p = VPath::parse("x/y").unwrap();

        assert_eq!(p.clone().into_path().unwrap(), p);
        assert_eq!((&p).into_path().unwrap(), p);
    }

    #[test]
    fn invalid_string_errors() {
        assert!("a//b".into_path().is_err());
    }
}
