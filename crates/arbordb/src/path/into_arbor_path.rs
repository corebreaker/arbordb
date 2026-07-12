//! Conversion into an [`APath`] for access-path-addressed APIs.

use super::APath;
use crate::error::AdbResult;

/// A value accepted where an access path (a whole-value, filesystem-like address)
/// is expected: an owned or borrowed [`APath`] (used as-is) or a string (parsed,
/// with the same normalization as [`APath::parse`]).
///
/// Filesystem methods (`store` / `load` / `fetch` / `ls` / `mv` / `cp` / `rm` /
/// `mkdir` / `kind` / `exists` / `rooted`, and the file side of `get`) take
/// `impl IntoArborPath`, so a literal like `"users/alice"` and an already-built
/// [`APath`] are both accepted without a separate overload. Parsing is fallible —
/// a malformed string (or one carrying a list index) surfaces as an
/// [`InvalidPath`](crate::AdbError::InvalidPath) error — while an [`APath`] passes
/// through unparsed. It is the access-side counterpart of
/// [`IntoValuePath`](super::IntoValuePath).
pub trait IntoArborPath {
    /// Converts `self` into an [`APath`], parsing it if it is a string.
    fn into_arbor_path(self) -> AdbResult<APath>;
}

impl IntoArborPath for APath {
    fn into_arbor_path(self) -> AdbResult<APath> {
        Ok(self)
    }
}

impl IntoArborPath for &APath {
    fn into_arbor_path(self) -> AdbResult<APath> {
        Ok(self.clone())
    }
}

impl IntoArborPath for &str {
    fn into_arbor_path(self) -> AdbResult<APath> {
        self.parse()
    }
}

impl IntoArborPath for String {
    fn into_arbor_path(self) -> AdbResult<APath> {
        self.parse()
    }
}

impl IntoArborPath for &String {
    fn into_arbor_path(self) -> AdbResult<APath> {
        self.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_forms_parse() {
        let want = APath::parse("users/alice").unwrap();

        assert_eq!("users/alice".into_arbor_path().unwrap(), want);
        assert_eq!(String::from("users/alice").into_arbor_path().unwrap(), want);
        assert_eq!((&String::from("users/alice")).into_arbor_path().unwrap(), want);
    }

    #[test]
    fn apath_forms_pass_through() {
        let p = APath::parse("a/b").unwrap();

        assert_eq!(p.clone().into_arbor_path().unwrap(), p);
        assert_eq!((&p).into_arbor_path().unwrap(), p);
    }

    #[test]
    fn invalid_string_errors() {
        // An access path carries no list index, and rejects empty segments.
        assert!("users/alice[0]".into_arbor_path().is_err());
        assert!("a//b".into_arbor_path().is_err());
    }
}
