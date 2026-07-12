//! A vnode's access-control list: its owning user, the [`Rights`] it grants that
//! owner, the rights it grants the members of each of its groups, and the rights it
//! grants everyone else.
//!
//! Rights are graded (`None` ⊂ `Access` ⊂ `Modify` ⊂ `Delete`; see [`Rights`]).
//! A vnode belongs to zero or more groups — the keys of its group map — and a caller
//! in several of them gets the strongest grade any of those groups is granted.
//! Group ids are kept in a [`BTreeMap`], so the encoding is deterministic: the value
//! integrity tags bind the encoded ACL, and a nondeterministic order would break them.
//! Changing an ACL needs `Modify` on the vnode — there is no separate admin right.

use crate::{acl::Rights, codec::Reader, error::AdbResult};
use std::collections::BTreeMap;

/// A group identifier, as stored in a vnode's ACL and the group store.
pub(crate) type GroupId = u32;

/// A vnode's access-control list. Every vnode has an owning user; it may grant
/// rights to any number of groups (a caller in several gets the strongest), and to
/// everyone else.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Acl {
    /// The owning user's id.
    owner_uid: u32,

    /// The rights granted to the owner.
    owner: Rights,

    /// The rights granted to each group the vnode belongs to, keyed by group id.
    group: BTreeMap<GroupId, Rights>,

    /// The rights granted to everyone else.
    other: Rights,
}

impl Acl {
    /// The default ACL for a freshly created vnode owned by `owner_uid`: the owner
    /// may delete it, everyone else may access (read + traverse) it, and it belongs
    /// to no group.
    pub(crate) fn default_for(owner_uid: u32) -> Self {
        Self {
            owner_uid,
            owner: Rights::Delete,
            group: BTreeMap::new(),
            other: Rights::Access,
        }
    }

    /// The owning user's id.
    pub(crate) fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    /// Reassigns the owning user (chown).
    pub(crate) fn set_owner_uid(&mut self, uid: u32) {
        self.owner_uid = uid;
    }

    /// The rights granted to the owner.
    pub(crate) fn owner_rights(&self) -> Rights {
        self.owner
    }

    /// The rights granted to everyone else.
    pub(crate) fn other_rights(&self) -> Rights {
        self.other
    }

    /// Sets the owner's rights.
    pub(crate) fn set_owner_rights(&mut self, rights: Rights) {
        self.owner = rights;
    }

    /// Sets everyone else's rights.
    pub(crate) fn set_other_rights(&mut self, rights: Rights) {
        self.other = rights;
    }

    /// The rights granted to `gid`, or [`Rights::None`] if the vnode is not in that
    /// group.
    pub(crate) fn group_rights(&self, gid: GroupId) -> Rights {
        self.group.get(&gid).copied().unwrap_or(Rights::None)
    }

    /// Grants `rights` to `gid`, adding the group if it is absent; [`Rights::None`]
    /// removes the group entirely.
    pub(crate) fn set_group_rights(&mut self, gid: GroupId, rights: Rights) {
        if rights == Rights::None {
            self.group.remove(&gid);
        } else {
            self.group.insert(gid, rights);
        }
    }

    /// Adds `gid` with the default [`Access`](Rights::Access) grade, unless it is
    /// already present (in which case its grade is kept).
    pub(crate) fn add_group(&mut self, gid: GroupId) {
        self.group.entry(gid).or_insert(Rights::Access);
    }

    /// Removes `gid` from the vnode's groups (a no-op if it is absent).
    pub(crate) fn remove_group(&mut self, gid: GroupId) {
        self.group.remove(&gid);
    }

    /// Whether the vnode belongs to `gid`.
    pub(crate) fn has_group(&self, gid: GroupId) -> bool {
        self.group.contains_key(&gid)
    }

    /// The ids of the groups the vnode belongs to, in ascending order.
    pub(crate) fn group_ids(&self) -> impl Iterator<Item = GroupId> + '_ {
        self.group.keys().copied()
    }

    /// The effective grade for a caller with id `uid` who is in groups `gids`: the
    /// owner's rights if they own the vnode, otherwise the strongest grade of any
    /// group they share with it, otherwise everyone-else's rights.
    pub(crate) fn effective_rights(&self, uid: u32, gids: &[u32]) -> Rights {
        if uid == self.owner_uid {
            return self.owner;
        }

        let grouped = gids.iter().filter_map(|gid| self.group.get(gid).copied()).max();

        grouped.unwrap_or(self.other)
    }

    /// Serializes the ACL: `owner_uid` (`u32`), the owner and other grades (one byte
    /// each), then a `u32` count and that many `(gid: u32, grade: u8)` pairs in
    /// ascending gid order.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 1 + 1 + 4 + self.group.len() * 5);
        out.extend_from_slice(&self.owner_uid.to_be_bytes());
        out.push(self.owner.to_bits());
        out.push(self.other.to_bits());
        out.extend_from_slice(&(self.group.len() as u32).to_be_bytes());

        for (gid, rights) in &self.group {
            out.extend_from_slice(&gid.to_be_bytes());
            out.push(rights.to_bits());
        }

        out
    }

    /// Decodes the layout [`encode`](Self::encode) produces.
    pub(crate) fn decode(body: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(body);
        let owner_uid = r.u32()?;
        let owner = Rights::from_bits(r.u8()?);
        let other = Rights::from_bits(r.u8()?);

        let count = r.u32()?;
        let mut group = BTreeMap::new();
        for _ in 0..count {
            let gid = r.u32()?;
            let rights = Rights::from_bits(r.u8()?);
            group.insert(gid, rights);
        }

        Ok(Self {
            owner_uid,
            owner,
            group,
            other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grades_are_cumulative_and_encode_round_trips() {
        assert!(Rights::Delete.includes(Rights::Modify));
        assert!(Rights::Modify.includes(Rights::Access));
        assert!(!Rights::Access.includes(Rights::Modify));

        for grade in [Rights::None, Rights::Access, Rights::Modify, Rights::Delete] {
            assert_eq!(Rights::from_bits(grade.to_bits()), grade);
        }
    }

    #[test]
    fn effective_rights_follow_owner_then_group_then_other() {
        let mut group = BTreeMap::new();
        group.insert(7u32, Rights::Access);
        group.insert(8u32, Rights::Modify);

        let acl = Acl {
            owner_uid: 1,
            owner: Rights::Delete,
            group,
            other: Rights::None,
        };

        // The owner gets the owner grade.
        assert_eq!(acl.effective_rights(1, &[7, 8]), Rights::Delete);

        // A non-owner in several groups gets the strongest of them.
        assert_eq!(acl.effective_rights(2, &[7, 8]), Rights::Modify);
        assert_eq!(acl.effective_rights(2, &[7]), Rights::Access);

        // A non-owner in none of the groups falls to other.
        assert_eq!(acl.effective_rights(2, &[9]), Rights::None);
    }

    #[test]
    fn setting_a_group_to_none_removes_it() {
        let mut acl = Acl::default_for(1);
        acl.set_group_rights(5, Rights::Modify);
        assert!(acl.has_group(5));

        acl.set_group_rights(5, Rights::None);
        assert!(!acl.has_group(5));
    }

    #[test]
    fn encode_decode_preserves_groups() {
        let mut acl = Acl::default_for(42);
        acl.set_group_rights(3, Rights::Modify);
        acl.set_group_rights(9, Rights::Delete);

        let decoded = Acl::decode(&acl.encode()).unwrap();
        assert_eq!(decoded, acl);
        assert_eq!(decoded.group_ids().collect::<Vec<_>>(), vec![3, 9]);
    }
}
