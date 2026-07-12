//! Conversion into a [`VPath`] for value-path-addressed APIs.

use super::VPath;
use crate::error::AdbResult;

/// A value accepted where an intra-value path is expected: an owned or borrowed
/// [`VPath`] (used as-is) or a string (parsed, with the same normalization as
/// [`VPath::parse`]).
///
/// Value-addressed methods (a scalar's `at` position, [`Value::get_value`] and its
/// siblings) take `impl IntoValuePath`, so a literal like `"a/b[2]"` and an
/// already-built [`VPath`] are both accepted without a separate overload. Parsing
/// is fallible — a malformed string surfaces as an
/// [`InvalidPath`](crate::AdbError::InvalidPath) error — while a [`VPath`] passes
/// through unparsed. It is the value-side counterpart of
/// [`IntoArborPath`](super::IntoArborPath).
///
/// [`Value::get_value`]: crate::Value::get_value
pub trait IntoValuePath {
    /// Converts `self` into a [`VPath`], parsing it if it is a string.
    fn into_value_path(self) -> AdbResult<VPath>;
}

impl IntoValuePath for VPath {
    fn into_value_path(self) -> AdbResult<VPath> {
        Ok(self)
    }
}

impl IntoValuePath for &VPath {
    fn into_value_path(self) -> AdbResult<VPath> {
        Ok(self.clone())
    }
}

impl IntoValuePath for &str {
    fn into_value_path(self) -> AdbResult<VPath> {
        self.parse()
    }
}

impl IntoValuePath for String {
    fn into_value_path(self) -> AdbResult<VPath> {
        self.parse()
    }
}

impl IntoValuePath for &String {
    fn into_value_path(self) -> AdbResult<VPath> {
        self.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_forms_parse() {
        let want = VPath::parse("a/b[2]/c").unwrap();

        assert_eq!("a/b[2]/c".into_value_path().unwrap(), want);
        assert_eq!(String::from("a/b[2]/c").into_value_path().unwrap(), want);
        assert_eq!((&String::from("a/b[2]/c")).into_value_path().unwrap(), want);
    }

    #[test]
    fn vpath_forms_pass_through() {
        let p = VPath::parse("x/y").unwrap();

        assert_eq!(p.clone().into_value_path().unwrap(), p);
        assert_eq!((&p).into_value_path().unwrap(), p);
    }

    #[test]
    fn invalid_string_errors() {
        assert!("a//b".into_value_path().is_err());
    }
}
