//! The dynamic, in-memory document type, [`Value`].
//!
//! `Value` is the structure ArborDb stores: a tree of `Leaf(Scalar)` / `List` /
//! `Node`, *faithful* in that each leaf keeps its exact [`Scalar`]. It is the one
//! dynamic value type; a value is serialized into a single blob (one redb entry)
//! and read back — losslessly — through the value codec.
//!
//! Beyond the in-memory accessors (`leaf` / `list` / `vnode`, `get` / `at`,
//! `push` / `insert` / `merge`, …) it carries path-addressed
//! [`get_value`](Value::get_value) / [`set_value`](Value::set_value), which create
//! containers as needed and never silently destroy data along the way.

use crate::{
    data::Scalar,
    path::{IntoValuePath, Segment, VPath},
};

use std::{collections::BTreeMap, mem::replace};

#[cfg(feature = "serde")]
use crate::error::AdbResult;

/// A dynamic document: a scalar leaf, an ordered list, or a named map of values —
/// the faithful in-memory form of a stored value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A single scalar value.
    Leaf(Scalar),
    /// An ordered sequence of values, addressable by index.
    List(Vec<Value>),
    /// A map of named values, kept in sorted key order.
    Node(BTreeMap<String, Value>),
}

impl Value {
    /// A leaf holding `value`.
    pub fn new_leaf(value: Scalar) -> Self {
        Value::Leaf(value)
    }

    /// An empty list.
    pub fn new_empty_list() -> Self {
        Value::List(Vec::new())
    }

    /// An empty vnode (named map).
    pub fn new_empty_node() -> Self {
        Value::Node(BTreeMap::new())
    }

    /// A list of the given values.
    pub fn new_list(list: Vec<Value>) -> Self {
        Value::List(list)
    }

    /// A vnode wrapping the given map.
    pub fn new_node(node: BTreeMap<String, Value>) -> Self {
        Value::Node(node)
    }

    /// Maps any [`serde::Serialize`] value into a native `Value` tree — objects,
    /// lists, and scalar leaves — so it is indexable and navigable by `VPath`, just
    /// like a value built through [`AData`](crate::AData). The inverse of
    /// [`to_serde_value`](Self::to_serde_value).
    ///
    /// To store a Serde value, prefer
    /// [`WriteTxn::store_serde_value`](crate::txn::WriteTxn::store_serde_value), which
    /// encodes straight to the blob without this intermediate tree.
    #[cfg(feature = "serde")]
    pub fn from_serde_value<T: serde::Serialize + ?Sized>(v: &T) -> AdbResult<Self> {
        crate::serde::to_value(v)
    }

    /// Reconstructs a [`serde::de::DeserializeOwned`] value from this `Value` tree —
    /// the inverse of [`from_serde_value`](Self::from_serde_value).
    #[cfg(feature = "serde")]
    pub fn to_serde_value<T: serde::de::DeserializeOwned>(&self) -> AdbResult<T> {
        crate::serde::from_value(self)
    }

    /// The scalar, if this is a [`Leaf`](Value::Leaf).
    pub fn leaf(&self) -> Option<&Scalar> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            _ => None,
        }
    }

    /// A mutable reference to the scalar, if this is a [`Leaf`](Value::Leaf).
    pub fn leaf_mut(&mut self) -> Option<&mut Scalar> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            _ => None,
        }
    }

    /// The elements, if this is a [`List`](Value::List).
    pub fn list(&self) -> Option<&[Value]> {
        match self {
            Self::List(list) => Some(list),
            _ => None,
        }
    }

    /// A mutable reference to the element vector, if this is a [`List`](Value::List).
    pub fn list_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::List(list) => Some(list),
            _ => None,
        }
    }

    /// The entries, if this is a [`Node`](Value::Node).
    pub fn node(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Node(node) => Some(node),
            _ => None,
        }
    }

    /// A mutable reference to the entry map, if this is a [`Node`](Value::Node).
    pub fn node_mut(&mut self) -> Option<&mut BTreeMap<String, Value>> {
        match self {
            Self::Node(node) => Some(node),
            _ => None,
        }
    }

    /// The element at `index`, if this is a list reaching that far.
    pub fn at(&self, index: usize) -> Option<&Value> {
        match self {
            Self::List(list) => list.get(index),
            _ => None,
        }
    }

    /// A mutable reference to the element at `index`, if this is a list reaching
    /// that far.
    pub fn at_mut(&mut self, index: usize) -> Option<&mut Value> {
        match self {
            Self::List(list) => list.get_mut(index),
            _ => None,
        }
    }

    /// Whether this is a vnode containing `key`. `false` for a leaf or list.
    pub fn contains_key(&self, key: impl AsRef<str>) -> bool {
        if let Self::Node(node) = self {
            node.contains_key(key.as_ref())
        } else {
            false
        }
    }

    /// The value under `key`, if this is a vnode containing it.
    pub fn get(&self, key: impl AsRef<str>) -> Option<&Value> {
        match self {
            Self::Node(node) => node.get(key.as_ref()),
            _ => None,
        }
    }

    /// A mutable reference to the value under `key`, if this is a vnode containing
    /// it.
    pub fn get_mut(&mut self, key: impl AsRef<str>) -> Option<&mut Value> {
        match self {
            Self::Node(node) => node.get_mut(key.as_ref()),
            _ => None,
        }
    }

    /// The [`NodeKind`](crate::NodeKind) of this value's root.
    pub fn node_kind(&self) -> crate::NodeKind {
        match self {
            Value::Leaf(_) => crate::NodeKind::Leaf,
            Value::List(_) => crate::NodeKind::List,
            Value::Node(_) => crate::NodeKind::Object,
        }
    }

    /// The number of direct children: a list's element count, a vnode's entry
    /// count, and `0` for a leaf — a scalar has no children. Pairs with
    /// [`is_empty`](Self::is_empty).
    pub fn len(&self) -> usize {
        match self {
            Self::Leaf(_) => 0,
            Self::List(list) => list.len(),
            Self::Node(node) => node.len(),
        }
    }

    /// Whether this value has no direct children — an empty list or vnode. A leaf
    /// counts as empty (structurally, a scalar has no children), so use
    /// [`node_kind`](Self::node_kind) to tell a leaf apart from an empty container.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The field names of a [`Node`](Value::Node), in sorted order; an empty
    /// iterator for a leaf or a list (neither has named keys).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.node().into_iter().flatten().map(|(name, _)| name.as_str())
    }

    /// Empties the value in place: a leaf becomes [`Null`](Scalar::Null), a list
    /// or vnode drops its children (keeping its kind).
    pub fn clear(&mut self) {
        match self {
            Self::Leaf(scalar) => {
                *scalar = Scalar::Null;
            }
            Self::List(list) => {
                list.clear();
            }
            Self::Node(node) => node.clear(),
        }
    }

    /// Appends `value` if this is a list; a no-op on a leaf or vnode.
    pub fn push(&mut self, value: Value) {
        if let Self::List(list) = self {
            list.push(value);
        }
    }

    /// Inserts (or replaces) the `key` → `value` entry if this is a vnode; a no-op
    /// on a leaf or list.
    pub fn insert(&mut self, key: String, value: Value) {
        if let Self::Node(node) = self {
            node.insert(key, value);
        }
    }

    /// Removes the element at `index` if this is a list; a no-op on a leaf or
    /// vnode. Panics if `index` is out of bounds (see [`Vec::remove`]).
    pub fn remove_at(&mut self, index: usize) {
        if let Self::List(list) = self {
            list.remove(index);
        }
    }

    /// Removes the `key` entry if this is a vnode; a no-op on a leaf or list.
    pub fn remove_key(&mut self, key: impl AsRef<str>) {
        if let Self::Node(node) = self {
            node.remove(key.as_ref());
        }
    }

    /// Merges `with` into `self`, by kind:
    ///
    /// - a [`Leaf`](Value::Leaf) is replaced wholesale by `with`;
    /// - a [`List`](Value::List) extends with another list's elements, or pushes a non-list `with` as one more element;
    /// - a [`Node`](Value::Node) extends with another vnode (overwriting duplicate keys), or is replaced wholesale by a
    ///   non-vnode `with`.
    pub fn merge(&mut self, with: Self) {
        match self {
            Self::Leaf(_) => {
                let _ = replace(self, with);
            }
            Self::List(list) => match with {
                Self::List(with_list) => {
                    list.extend(with_list);
                }
                with => {
                    list.push(with);
                }
            },
            Self::Node(node) => match with {
                Self::Node(with_node) => {
                    node.extend(with_node);
                }
                with => {
                    let _ = replace(self, with);
                }
            },
        }
    }

    /// Returns a clone of the subtree at `path`, or `None` if no vnode sits there
    /// (or `path` does not parse). The root path returns the whole value. Use
    /// [`get_value_ref`](Self::get_value_ref) to borrow it without cloning.
    pub fn get_value(&self, path: impl IntoValuePath) -> Option<Value> {
        Some(self.get_value_ref(path)?.clone())
    }

    /// Borrows the subtree at `path` without cloning, or `None` if no vnode sits
    /// there (or `path` does not parse). The root path borrows the whole value.
    pub fn get_value_ref(&self, path: impl IntoValuePath) -> Option<&Value> {
        self.subtree(&path.into_value_path().ok()?)
    }

    /// Mutably borrows the subtree at `path`, or `None` if no vnode sits there (or
    /// `path` does not parse). The root path borrows the whole value. Unlike
    /// [`set_value`](Self::set_value) it creates nothing along the way — a path
    /// leading nowhere yields `None`.
    pub fn get_value_mut(&mut self, path: impl IntoValuePath) -> Option<&mut Value> {
        let base = path.into_value_path().ok()?;

        descend_mut(self, base.segments())
    }

    /// Sets `value` at `path`, creating missing containers on the way (a `Name`
    /// segment makes an object, an `Index` a list), and returns whether it applied.
    ///
    /// It never descends through or overwrites a leaf sitting *along* the path, and
    /// never grows a list past its end; on such a conflict (or a path that does not
    /// parse) it leaves `self` untouched and returns `false`. The value at the
    /// destination itself is replaced. The root path replaces the whole value.
    pub fn set_value(&mut self, path: impl IntoValuePath, value: Value) -> bool {
        let Ok(base) = path.into_value_path() else {
            return false;
        };

        set_in(self, base.segments(), value)
    }

    /// Removes the subtree at `path`, returning whether anything was removed. The
    /// root path resets the whole value to the [`Null`](Scalar::Null) default. A
    /// path that leads nowhere (or does not parse) is a no-op returning `false`.
    pub fn remove_value(&mut self, path: impl IntoValuePath) -> bool {
        let Ok(base) = path.into_value_path() else {
            return false;
        };

        remove_in(self, base.segments())
    }

    /// Borrows the subtree at `base`, following each segment (`Name` into objects,
    /// `Index` into lists); `None` if a segment leads nowhere.
    pub(crate) fn subtree(&self, base: &VPath) -> Option<&Value> {
        let mut current = self;
        for segment in base.segments() {
            current = match segment {
                Segment::Name(name) => current.get(name)?,
                Segment::Index(index) => current.at(*index as usize)?,
            };
        }

        Some(current)
    }
}

/// Places `value` at `segments` within `target`: descends into existing
/// containers, creates missing ones, and replaces the destination. Returns
/// `false` without mutating `target` if a segment would traverse a leaf, hit the
/// wrong container kind, or grow a list past its end.
fn set_in(target: &mut Value, segments: &[Segment], value: Value) -> bool {
    let Some((first, rest)) = segments.split_first() else {
        *target = value;

        return true;
    };

    match (target, first) {
        (Value::Node(map), Segment::Name(name)) => match map.get_mut(name.as_str()) {
            Some(child) => set_in(child, rest, value),
            None => match build_fresh(rest, value) {
                Some(subtree) => {
                    map.insert(name.to_string(), subtree);

                    true
                }
                None => false,
            },
        },
        (Value::List(list), Segment::Index(index)) => {
            let index = *index as usize;

            if index < list.len() {
                set_in(&mut list[index], rest, value)
            } else if index == list.len() {
                match build_fresh(rest, value) {
                    Some(subtree) => {
                        list.push(subtree);

                        true
                    }
                    None => false,
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

impl Default for Value {
    /// The empty default is a [`Null`](Scalar::Null) leaf.
    fn default() -> Self {
        Value::Leaf(Scalar::Null)
    }
}

/// Builds a brand-new subtree placing `value` at `segments`. `None` if it cannot
/// be created — an `Index` in fresh territory can only be `0`, since a new list
/// starts empty, so any higher index has no slot.
fn build_fresh(segments: &[Segment], value: Value) -> Option<Value> {
    let Some((first, rest)) = segments.split_first() else {
        return Some(value);
    };

    match first {
        Segment::Name(name) => {
            let mut map = BTreeMap::new();
            map.insert(name.to_string(), build_fresh(rest, value)?);

            Some(Value::Node(map))
        }
        Segment::Index(0) => Some(Value::List(vec![build_fresh(rest, value)?])),
        Segment::Index(_) => None,
    }
}

/// Removes the vnode at `segments` from `target`. An empty path resets `target` to
/// the default; otherwise it descends to the parent and drops the last segment.
fn remove_in(target: &mut Value, segments: &[Segment]) -> bool {
    let Some((last, head)) = segments.split_last() else {
        *target = Value::default();

        return true;
    };

    let Some(parent) = descend_mut(target, head) else {
        return false;
    };

    match last {
        Segment::Name(name) => match parent {
            Value::Node(map) => map.remove(name.as_str()).is_some(),
            _ => false,
        },
        Segment::Index(index) => match parent {
            Value::List(list) => {
                let index = *index as usize;
                if index < list.len() {
                    list.remove(index);

                    true
                } else {
                    false
                }
            }
            _ => false,
        },
    }
}

/// Descends `segments` into `target` by mutable reference; `None` if a segment
/// leads nowhere or hits the wrong container kind.
fn descend_mut<'a>(mut current: &'a mut Value, segments: &[Segment]) -> Option<&'a mut Value> {
    for segment in segments {
        current = match (current, segment) {
            (Value::Node(map), Segment::Name(name)) => map.get_mut(name.as_str())?,
            (Value::List(list), Segment::Index(index)) => list.get_mut(*index as usize)?,
            _ => return None,
        };
    }

    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(n: i64) -> Value {
        Value::new_leaf(Scalar::I64(n))
    }

    #[test]
    fn constructors_build_the_expected_shapes() {
        assert_eq!(Value::new_leaf(Scalar::Bool(true)), Value::Leaf(Scalar::Bool(true)));
        assert_eq!(Value::new_empty_list(), Value::List(vec![]));
        assert_eq!(Value::new_empty_node(), Value::Node(BTreeMap::new()));
        assert_eq!(Value::new_list(vec![leaf(1)]), Value::List(vec![leaf(1)]));

        let mut map = BTreeMap::new();
        map.insert(String::from("a"), leaf(1));

        assert_eq!(Value::new_node(map.clone()), Value::Node(map));
    }

    #[test]
    fn default_is_a_null_leaf() {
        assert_eq!(Value::default(), Value::Leaf(Scalar::Null));
    }

    #[test]
    fn typed_accessors_match_only_their_own_kind() {
        let mut lf = leaf(7);
        let mut ls = Value::new_list(vec![leaf(1)]);
        let mut nd = Value::new_empty_node();
        nd.insert(String::from("k"), leaf(2));

        assert_eq!(lf.leaf(), Some(&Scalar::I64(7)));
        assert!(ls.leaf().is_none());
        assert!(lf.leaf_mut().is_some());
        assert!(nd.leaf_mut().is_none());

        assert_eq!(ls.list().map(<[Value]>::len), Some(1));
        assert!(lf.list().is_none());
        assert!(ls.list_mut().is_some());
        assert!(lf.list_mut().is_none());

        assert_eq!(nd.node().map(BTreeMap::len), Some(1));
        assert!(lf.node().is_none());
        assert!(nd.node_mut().is_some());
        assert!(lf.node_mut().is_none());
    }

    #[test]
    fn indexed_and_keyed_access() {
        let ls = Value::new_list(vec![leaf(10), leaf(20)]);
        assert_eq!(ls.at(1), Some(&Value::Leaf(Scalar::I64(20))));
        assert!(ls.at(2).is_none());
        assert!(leaf(0).at(0).is_none());

        let mut ls = ls;
        *ls.at_mut(0).unwrap() = leaf(99);
        assert_eq!(ls.at(0), Some(&leaf(99)));
        assert!(ls.at_mut(9).is_none());
        assert!(leaf(0).at_mut(0).is_none());

        let mut nd = Value::new_empty_node();
        nd.insert(String::from("x"), leaf(5));
        assert!(nd.contains_key("x"));
        assert!(!nd.contains_key("y"));
        assert!(!leaf(0).contains_key("x"));

        assert_eq!(nd.get("x"), Some(&leaf(5)));
        assert!(nd.get("y").is_none());
        assert!(leaf(0).get("x").is_none());

        *nd.get_mut("x").unwrap() = leaf(6);
        assert_eq!(nd.get("x"), Some(&leaf(6)));
        assert!(nd.get_mut("y").is_none());
        assert!(leaf(0).get_mut("x").is_none());
    }

    #[test]
    fn clear_empties_each_kind_keeping_it() {
        let mut lf = leaf(3);
        lf.clear();
        assert_eq!(lf, Value::Leaf(Scalar::Null));

        let mut ls = Value::new_list(vec![leaf(1), leaf(2)]);
        ls.clear();
        assert_eq!(ls, Value::new_empty_list());

        let mut nd = Value::new_empty_node();
        nd.insert(String::from("k"), leaf(1));
        nd.clear();
        assert_eq!(nd, Value::new_empty_node());
    }

    #[test]
    fn push_insert_remove_are_kind_gated_no_ops() {
        let mut ls = Value::new_empty_list();
        ls.push(leaf(1));
        assert_eq!(ls.list().map(<[Value]>::len), Some(1));

        // No-op on the wrong kind.
        let mut lf = leaf(0);
        lf.push(leaf(2));
        assert_eq!(lf, leaf(0));

        let mut nd = Value::new_empty_node();
        nd.insert(String::from("a"), leaf(1));
        assert!(nd.contains_key("a"));

        lf.insert(String::from("a"), leaf(1));
        assert_eq!(lf, leaf(0));

        ls.remove_at(0);
        assert_eq!(ls, Value::new_empty_list());
        lf.remove_at(0); // no-op on a leaf

        nd.remove_key("a");
        assert!(!nd.contains_key("a"));
        lf.remove_key("a"); // no-op on a leaf
        assert_eq!(lf, leaf(0));
    }

    #[test]
    fn merge_by_kind() {
        // Leaf is replaced wholesale.
        let mut lf = leaf(1);
        lf.merge(Value::new_list(vec![leaf(2)]));
        assert_eq!(lf, Value::new_list(vec![leaf(2)]));

        // List extends with another list, or pushes a non-list.
        let mut ls = Value::new_list(vec![leaf(1)]);
        ls.merge(Value::new_list(vec![leaf(2), leaf(3)]));
        assert_eq!(ls.list().map(<[Value]>::len), Some(3));
        ls.merge(leaf(4));
        assert_eq!(ls.list().map(<[Value]>::len), Some(4));

        // Node extends with another vnode, or is replaced by a non-vnode.
        let mut a = Value::new_empty_node();
        a.insert(String::from("x"), leaf(1));
        let mut b = Value::new_empty_node();
        b.insert(String::from("x"), leaf(9));
        b.insert(String::from("y"), leaf(2));
        a.merge(b);
        assert_eq!(a.get("x"), Some(&leaf(9)));
        assert_eq!(a.get("y"), Some(&leaf(2)));

        a.merge(leaf(0));
        assert_eq!(a, leaf(0));
    }

    #[test]
    fn get_value_navigates_clones_and_rejects_bad_paths() {
        let mut root = Value::new_empty_node();
        root.set_value("a/b", leaf(42));

        assert_eq!(root.get_value("a/b"), Some(leaf(42)));
        assert_eq!(root.get_value(""), Some(root.clone()));
        assert!(root.get_value("a/missing").is_none());
        assert!(root.get_value("a/b/c").is_none()); // through a leaf

        let ls = Value::new_list(vec![leaf(1), leaf(2)]);
        assert_eq!(ls.get_value("[1]"), Some(leaf(2)));
        assert!(ls.get_value("[5]").is_none());
    }

    #[test]
    fn len_is_empty_and_keys_by_kind() {
        // A leaf has no children: len 0, empty, no keys.
        let lf = leaf(7);
        assert_eq!(lf.len(), 0);
        assert!(lf.is_empty());
        assert!(lf.keys().next().is_none());

        // A list counts its elements and has no named keys.
        let ls = Value::new_list(vec![leaf(1), leaf(2), leaf(3)]);
        assert_eq!(ls.len(), 3);
        assert!(!ls.is_empty());
        assert!(Value::new_empty_list().is_empty());
        assert!(ls.keys().next().is_none());

        // A vnode counts its entries and lists its keys in sorted order.
        let mut nd = Value::new_empty_node();
        nd.insert(String::from("b"), leaf(1));
        nd.insert(String::from("a"), leaf(2));
        assert_eq!(nd.len(), 2);
        assert!(!nd.is_empty());
        assert!(Value::new_empty_node().is_empty());
        assert_eq!(nd.keys().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn get_value_ref_and_mut_borrow_without_cloning() {
        let mut root = Value::new_empty_node();
        root.set_value("a/b", leaf(1));

        // A borrow reaches the same vnode the cloning getter returns.
        assert_eq!(root.get_value_ref("a/b"), Some(&leaf(1)));
        let whole = root.clone();
        assert_eq!(root.get_value_ref(""), Some(&whole));
        assert!(root.get_value_ref("a/missing").is_none());
        assert!(root.get_value_ref("a/b/c").is_none()); // through a leaf

        // A mutable borrow edits in place and creates nothing along the way.
        *root.get_value_mut("a/b").unwrap() = leaf(9);
        assert_eq!(root.get_value("a/b"), Some(leaf(9)));
        assert!(root.get_value_mut("a/missing").is_none());
        assert_eq!(root.get_value("a/b"), Some(leaf(9))); // the failed lookup changed nothing

        // The root path borrows the whole value.
        root.get_value_mut("").unwrap().clear();
        assert!(root.is_empty());
    }

    #[test]
    fn set_value_creates_containers() {
        let mut root = Value::new_empty_node();

        assert!(root.set_value("a/b/c", leaf(1)));
        assert_eq!(root.get_value("a/b/c"), Some(leaf(1)));

        // A fresh list only accepts index 0.
        let mut n = Value::new_empty_node();
        assert!(n.set_value("list[0]", leaf(7)));
        assert_eq!(n.get_value("list[0]"), Some(leaf(7)));

        let mut n2 = Value::new_empty_node();
        assert!(!n2.set_value("list[1]", leaf(7)));
        assert_eq!(n2, Value::new_empty_node());
    }

    #[test]
    fn set_value_appends_at_the_list_end_but_not_past_it() {
        let mut root = Value::new_empty_node();
        root.set_value("xs[0]", leaf(1));

        assert!(root.set_value("xs[1]", leaf(2))); // append at end
        assert_eq!(root.get_value("xs[1]"), Some(leaf(2)));

        assert!(!root.set_value("xs[5]", leaf(9))); // past the end
        assert_eq!(root.get_value("xs[5]"), None);
    }

    #[test]
    fn set_value_never_destroys_a_leaf_mid_path() {
        let mut root = Value::new_empty_node();
        root.set_value("a", leaf(1));

        // "a" is a leaf; descending through it must fail and change nothing.
        assert!(!root.set_value("a/b", leaf(2)));
        assert_eq!(root.get_value("a"), Some(leaf(1)));

        // Wrong container kind: indexing into a vnode, naming into a list.
        assert!(!root.set_value("a[0]", leaf(3)));
        assert_eq!(root.get_value("a"), Some(leaf(1)));
    }

    #[test]
    fn set_value_at_root_replaces_everything() {
        let mut root = Value::new_empty_node();
        root.set_value("a", leaf(1));

        assert!(root.set_value("", leaf(0)));
        assert_eq!(root, leaf(0));
    }

    #[test]
    fn set_value_rejects_an_unparsable_path() {
        let mut root = Value::new_empty_node();
        assert!(!root.set_value("a[", leaf(1)));
    }

    #[test]
    fn set_value_replaces_the_destination_leaf() {
        let mut root = Value::new_empty_node();
        root.set_value("a", leaf(1));

        assert!(root.set_value("a", leaf(2)));
        assert_eq!(root.get_value("a"), Some(leaf(2)));
    }

    #[test]
    fn set_value_descends_into_and_guards_list_elements() {
        let mut root = Value::new_empty_node();

        // Seed a one-element list whose element is an object.
        assert!(root.set_value("xs[0]/inner", leaf(1)));
        // Descend into the existing element to add a sibling field.
        assert!(root.set_value("xs[0]/other", leaf(2)));
        assert_eq!(root.get_value("xs[0]/inner"), Some(leaf(1)));
        assert_eq!(root.get_value("xs[0]/other"), Some(leaf(2)));

        // Appending at the list end with an unbuildable remainder (a non-zero index
        // into a fresh list) fails without mutating.
        assert!(!root.set_value("xs[1][5]", leaf(3)));
        assert_eq!(root.get_value("xs[1]"), None);
    }

    #[test]
    fn node_kind_reflects_the_root_shape() {
        assert_eq!(leaf(1).node_kind(), crate::NodeKind::Leaf);
        assert_eq!(Value::new_empty_list().node_kind(), crate::NodeKind::List);
        assert_eq!(Value::new_empty_node().node_kind(), crate::NodeKind::Object);
    }

    #[test]
    fn remove_value_drops_subtrees_by_kind() {
        let mut root = Value::new_empty_node();
        root.set_value("a/b", leaf(1));
        root.set_value("a/c", leaf(2));
        root.set_value("xs[0]/field", leaf(10)); // xs[0] is an object
        root.set_value("xs[1]", leaf(20));

        // Remove a named entry, descending through "a".
        assert!(root.remove_value("a/b"));
        assert!(root.get_value("a/b").is_none());
        assert_eq!(root.get_value("a/c"), Some(leaf(2)));

        // Descend through a list index to remove a nested field.
        assert!(root.remove_value("xs[0]/field"));
        assert!(root.get_value("xs[0]/field").is_none());

        // Remove a list element by index.
        assert!(root.remove_value("xs[1]"));
        assert!(root.get_value("xs[1]").is_none());

        // Out-of-range index: a no-op.
        assert!(!root.remove_value("xs[9]"));

        // Wrong kind: a name into a list, an index into a vnode.
        assert!(!root.remove_value("xs/name"));
        assert!(!root.remove_value("a[0]"));

        // A path that leads nowhere.
        assert!(!root.remove_value("nope/here"));

        // An unparsable path is a no-op.
        assert!(!root.remove_value("a["));

        // The root path resets to the default null leaf.
        assert!(root.remove_value(""));
        assert_eq!(root, Value::default());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_value_bridge_round_trips() {
        let value = Value::from_serde_value(&7i64).unwrap();
        assert_eq!(value, Value::Leaf(Scalar::I64(7)));

        let back: i64 = value.to_serde_value().unwrap();
        assert_eq!(back, 7);
    }
}
