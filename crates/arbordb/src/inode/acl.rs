//! A vnode's access-control list: an owner, an optional group, and mode bits
//! granting read/write — and, for directories, walk — to each of owner/group/other.
//!
//! The mode packs 3 rights × 3 classes into a `u16`, class-major, so the bit for
//! `(class, right)` is `class_index * 3 + right_index`. The walk bit is inert for
//! files (checked only on directories). Changing an ACL needs `write` on the vnode
//! — there is no separate admin right.

use crate::{codec::Reader, error::AdbResult};

/// The sentinel group value meaning "no group" on disk.
const NO_GROUP: u32 = u32::MAX;

/// A right that can be checked on a vnode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Right {
    /// Read a file's value, or list a directory's children.
    Read,
    /// Write a file's value or ACL, or add/remove/rename a directory's children.
    Write,
    /// Traverse *through* a directory to reach a descendant (directories only).
    Walk,
}

/// Which of owner/group/other a principal falls into for a given vnode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Class {
    /// The vnode's owner.
    Owner,
    /// A member of the vnode's group.
    Group,
    /// Everyone else.
    Other,
}

impl Right {
    fn index(self) -> u16 {
        match self {
            Right::Read => 0,
            Right::Write => 1,
            Right::Walk => 2,
        }
    }
}

impl Class {
    fn index(self) -> u16 {
        match self {
            Class::Owner => 0,
            Class::Group => 1,
            Class::Other => 2,
        }
    }
}

/// The mode bit granting `right` to `class`.
fn bit(class: Class, right: Right) -> u16 {
    1 << (class.index() * 3 + right.index())
}

/// Packs the public per-class rights into the on-disk mode bits.
pub(crate) fn mode_to_bits(mode: &crate::acl::Mode) -> u16 {
    let class_bits = |class: Class, rights: &crate::acl::Rights| {
        let mut bits = 0;
        if rights.read {
            bits |= bit(class, Right::Read);
        }
        if rights.write {
            bits |= bit(class, Right::Write);
        }
        if rights.walk {
            bits |= bit(class, Right::Walk);
        }

        bits
    };

    class_bits(Class::Owner, &mode.owner)
        | class_bits(Class::Group, &mode.group)
        | class_bits(Class::Other, &mode.other)
}

/// A vnode's access-control list. Every vnode has an owner; the group is optional
/// (a vnode with no group treats every non-owner as `other`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Acl {
    /// The owner's user id.
    owner: u32,
    /// The group's id, or `None` for a vnode with no group.
    group: Option<u32>,
    /// The packed rights (3 rights × 3 classes; see the module docs).
    mode:  u16,
}

impl Acl {
    /// An ACL with an explicit owner, optional group, and mode.
    pub(crate) fn new(owner: u32, group: Option<u32>, mode: u16) -> Self {
        Self {
            owner,
            group,
            mode,
        }
    }

    /// The default mode for a freshly created vnode: the owner gets every right;
    /// group and other get read + walk (so paths stay traversable and listable).
    pub(crate) fn default_mode() -> u16 {
        bit(Class::Owner, Right::Read)
            | bit(Class::Owner, Right::Write)
            | bit(Class::Owner, Right::Walk)
            | bit(Class::Group, Right::Read)
            | bit(Class::Group, Right::Walk)
            | bit(Class::Other, Right::Read)
            | bit(Class::Other, Right::Walk)
    }

    /// The owner's user id.
    pub(crate) fn owner(&self) -> u32 {
        self.owner
    }

    /// The group's id, if the vnode has one.
    pub(crate) fn group(&self) -> Option<u32> {
        self.group
    }

    /// Reassigns the owner (chown).
    pub(crate) fn set_owner(&mut self, owner: u32) {
        self.owner = owner;
    }

    /// Reassigns the group, or clears it (chgrp).
    pub(crate) fn set_group(&mut self, group: Option<u32>) {
        self.group = group;
    }

    /// Replaces the mode bits (chmod).
    pub(crate) fn set_mode(&mut self, mode: u16) {
        self.mode = mode;
    }

    /// The mode as the public per-class rights.
    pub(crate) fn to_mode(self) -> crate::acl::Mode {
        let rights = |class: Class| crate::acl::Rights {
            read:  self.allows(class, Right::Read),
            write: self.allows(class, Right::Write),
            walk:  self.allows(class, Right::Walk),
        };

        crate::acl::Mode {
            owner: rights(Class::Owner),
            group: rights(Class::Group),
            other: rights(Class::Other),
        }
    }

    /// Whether `class` is granted `right`.
    pub(crate) fn allows(&self, class: Class, right: Right) -> bool {
        self.mode & bit(class, right) != 0
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(10);
        out.extend_from_slice(&self.owner.to_be_bytes());
        out.extend_from_slice(&self.group.unwrap_or(NO_GROUP).to_be_bytes());
        out.extend_from_slice(&self.mode.to_be_bytes());

        out
    }

    pub(crate) fn decode(body: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(body);
        let owner = r.u32()?;
        let group = match r.u32()? {
            NO_GROUP => None,
            gid => Some(gid),
        };
        let mode = u16::from_be_bytes(r.array::<2>()?);

        Ok(Self {
            owner,
            group,
            mode,
        })
    }
}
