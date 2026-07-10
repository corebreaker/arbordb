//! Public access-control types for the `permissions` feature.
//!
//! [`Rights`](crate::acl::Rights) is a single *graded* right — `None` ⊂ `Access` ⊂ `Modify` ⊂ `Delete`,
//! each grade including every weaker one. It is what
//! [`WriteTxn::set_acl`](crate::txn::WriteTxn::set_acl) grants and what
//! [`ReadTxn::get_acl`](crate::txn::ReadTxn::get_acl) reports. [`AclClass`](crate::acl::AclClass) selects
//! *which* class of a vnode's ACL — its owner, one of its groups, or everyone else —
//! an operation reads or sets. There is no longer a separate `walk` right: a
//! directory is traversable by anyone who holds `Access` on it.

/// A graded access right on a vnode.
///
/// The grades are cumulative — each includes every weaker one — so an ordering
/// comparison answers "does this grade allow that one?": `Delete` > `Modify` >
/// `Access` > `None`. Concretely:
///
/// - [`Access`](Rights::Access) — read a file's value, list a directory, and traverse a directory to reach a
///   descendant.
/// - [`Modify`](Rights::Modify) — everything `Access` grants, plus overwrite a file's value or a vnode's ACL, and add /
///   remove / rename a directory's children.
/// - [`Delete`](Rights::Delete) — everything `Modify` grants, plus delete the vnode itself.
///
/// On disk each grade is a 3-bit cumulative mask (`Access` = 1, `Modify` = 3,
/// `Delete` = 7), so a stronger grade's bits cover every weaker grade's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rights {
    /// No access at all.
    #[default]
    None,

    /// Read a file's value, list a directory, and traverse a directory.
    Access,

    /// Everything [`Access`](Rights::Access) grants, plus overwrite a file's value
    /// or a vnode's ACL, and add / remove / rename a directory's children.
    Modify,

    /// Everything [`Modify`](Rights::Modify) grants, plus delete the vnode.
    Delete,
}

impl Rights {
    /// The 3-bit cumulative on-disk encoding of this grade (`None` = 0,
    /// `Access` = 1, `Modify` = 3, `Delete` = 7).
    pub(crate) fn to_bits(self) -> u8 {
        match self {
            Rights::None => 0,
            Rights::Access => 1,
            Rights::Modify => 3,
            Rights::Delete => 7,
        }
    }

    /// Decodes a grade from its 3-bit cumulative encoding, taking the strongest
    /// grade whose bits are all present so an unknown pattern degrades safely.
    pub(crate) fn from_bits(bits: u8) -> Self {
        if bits & 0b111 == 0b111 {
            Rights::Delete
        } else if bits & 0b011 == 0b011 {
            Rights::Modify
        } else if bits & 0b001 == 0b001 {
            Rights::Access
        } else {
            Rights::None
        }
    }

    /// Whether this grade includes `needed` (grades are cumulative, so this is
    /// simply `self >= needed`).
    pub(crate) fn includes(self, needed: Rights) -> bool {
        self >= needed
    }
}

/// Selects one class of a vnode's access-control list.
///
/// A vnode grants rights to its owner, to the members of any number of named
/// groups, and to everyone else. [`get_acl`](crate::txn::ReadTxn::get_acl) reads,
/// and [`set_acl`](crate::txn::WriteTxn::set_acl) sets, the [`Rights`] of the class
/// named here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AclClass {
    /// The vnode's owner.
    User,

    /// The members of the group with this name.
    Group(String),

    /// Everyone who is neither the owner nor a member of one of the vnode's groups.
    Other,
}
