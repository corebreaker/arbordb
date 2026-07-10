use super::VPath;

/// A value that can be appended to a [`VPath`] with `/` or `/=`.
///
/// A path tail (`VPath`/`&VPath`) joins segment-wise (like [`VPath::join`]); a
/// string tail (`&str`/`String`) appends a single field name (like
/// [`VPath::child_name`], with the same `.`/`..` handling). For an index or a
/// multi-segment fragment, parse the string first: `path / VPath::parse("t[0]")?`.
pub trait PathTail {
    /// Appends this value's segment(s) onto `path`.
    fn append_to(self, path: &mut VPath);
}

impl PathTail for VPath {
    fn append_to(self, path: &mut VPath) {
        path.inplace_join(&self);
    }
}

impl PathTail for &VPath {
    fn append_to(self, path: &mut VPath) {
        path.inplace_join(self);
    }
}

impl<S: AsRef<str>> PathTail for S {
    fn append_to(self, path: &mut VPath) {
        path.push_name(self.as_ref());
    }
}
