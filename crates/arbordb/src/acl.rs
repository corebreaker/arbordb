//! Public access-control types for the `permissions` feature.
//!
//! [`Mode`](crate::acl::Mode) describes the read/write/walk rights of each class
//! (owner, group, other); it is what [`WriteTxn::chmod`](crate::txn::WriteTxn::chmod)
//! takes and part of the [`NodeAcl`](crate::acl::NodeAcl) that
//! [`ReadTxn::get_acl`](crate::txn::ReadTxn::get_acl) returns. `walk` is meaningful
//! only for directories.

/// The rights granted to one class on a vnode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rights {
    /// Read a file's value, or list a directory's children.
    pub read: bool,

    /// Write a file's value or ACL, or add/remove/rename a directory's children.
    pub write: bool,

    /// Traverse *through* a directory to reach a descendant (directories only).
    pub walk: bool,
}

impl Rights {
    /// Rights granting only `read` (and nothing else) — a convenience constructor.
    pub fn read_only() -> Self {
        Self {
            read:  true,
            write: false,
            walk:  false,
        }
    }
}

/// A vnode's permission bits: the rights of each of owner, group, and other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    /// The owner's rights.
    pub owner: Rights,

    /// The group's rights (apply only when the vnode has a group the caller is in).
    pub group: Rights,

    /// Everyone else's rights.
    pub other: Rights,
}

/// A vnode's full access-control list, with owner and group resolved to names.
///
/// Returned by [`ReadTxn::get_acl`](crate::txn::ReadTxn::get_acl).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeAcl {
    /// The owning user's name.
    pub owner: String,

    /// The owning group's name, if the vnode has a group.
    pub group: Option<String>,

    /// The permission bits.
    pub mode: Mode,
}
