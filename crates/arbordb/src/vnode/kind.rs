/// The kind of a vnode inside a stored value, as reported by the public API.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    /// A map from field names to child nodes.
    Object,
    /// A zero-based sequence of child nodes.
    List,
    /// A single scalar value.
    Leaf,
}
