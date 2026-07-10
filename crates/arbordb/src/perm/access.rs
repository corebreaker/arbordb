//! The value-access authorizer: may a principal exercise a right on a vnode?
//!
//! - The **master user** and an **unrestricted** (non-protected) handle bypass ACLs.
//! - The **guest** is strictly read-only: any write is refused whatever the ACL says.
//! - The **root** vnode is special — everyone may read and walk it, any non-guest may create children in it, and its
//!   ACL is immutable (never stored).
//! - A vnode with **no ACL** (e.g. one predating protection) is reachable only by the master user or a **master-group**
//!   member.
//! - Otherwise the identity's owner/group/other class is checked against the ACL. Membership of the master or super
//!   *group* grants no bypass here.

use super::Principal;
use crate::{
    error::{AdbError, AdbResult},
    inode::{Acl, Class, Right},
    AKey,
};

/// Checks that `principal` may exercise `right` on vnode `akey` governed by `acl`
/// (`None` when the vnode has no ACL yet).
pub(crate) fn authorize(principal: &Principal, akey: AKey, acl: Option<&Acl>, right: Right) -> AdbResult<()> {
    // The guest is read-only, whatever the ACL or the vnode says.
    if matches!(principal, Principal::Guest) && right == Right::Write {
        return Err(denied(right));
    }

    // The root is special: read/walk for everyone, create-children for any
    // non-guest (already screened above), and no ACL to consult.
    if akey == AKey::ROOT {
        return Ok(());
    }

    match principal {
        Principal::Unrestricted => return Ok(()),
        Principal::User(session) if session.is_master() => return Ok(()),
        _ => {}
    }

    match acl {
        // A vnode with no ACL is reachable only by the master user (handled above)
        // or a master-group member.
        None => {
            if matches!(principal, Principal::User(session) if session.in_master_group()) {
                Ok(())
            } else {
                Err(denied(right))
            }
        }
        Some(acl) => class_check(principal, acl, right),
    }
}

/// Resolves the identity's class for `acl` and checks it holds `right`.
fn class_check(principal: &Principal, acl: &Acl, right: Right) -> AdbResult<()> {
    let (uid, gids): (u32, &[u32]) = match principal {
        Principal::User(session) => (session.uid(), session.gids()),
        Principal::Guest => (super::GUEST_UID, &[]),
        // Unrestricted and the master user are handled by the caller.
        Principal::Unrestricted => return Ok(()),
    };

    if acl.allows(class_of(uid, gids, acl), right) {
        Ok(())
    } else {
        Err(denied(right))
    }
}

/// The owner/group/other class the identity `uid`/`gids` falls into for `acl`.
fn class_of(uid: u32, gids: &[u32], acl: &Acl) -> Class {
    if uid == acl.owner() {
        Class::Owner
    } else if acl.group().is_some_and(|gid| gids.contains(&gid)) {
        Class::Group
    } else {
        Class::Other
    }
}

/// Checks that `principal` may change a vnode's **owner**. The `write` right alone
/// is not enough — only the master user, a master-group member, or the current
/// owner may chown.
pub(crate) fn authorize_chown(principal: &Principal, acl: &Acl) -> AdbResult<()> {
    let allowed = match principal {
        Principal::Unrestricted => true,
        Principal::User(session) if session.is_master() || session.in_master_group() => true,
        Principal::User(session) => session.uid() == acl.owner(),
        Principal::Guest => false,
    };

    if allowed {
        Ok(())
    } else {
        Err(AdbError::PermissionDenied(String::from(
            "changing the owner requires ownership or administration",
        )))
    }
}

/// Checks that `principal` may set a vnode's **group** to `target` (`None` clears
/// it). The master user and master-group members may set any group; anyone else
/// must be able to write the vnode and, when setting a group, be a member of it.
pub(crate) fn authorize_chgrp(principal: &Principal, acl: &Acl, target: Option<u32>) -> AdbResult<()> {
    let (uid, gids): (u32, &[u32]) = match principal {
        Principal::Unrestricted => return Ok(()),
        Principal::User(session) if session.is_master() || session.in_master_group() => return Ok(()),
        Principal::User(session) => (session.uid(), session.gids()),
        Principal::Guest => return Err(denied_chgrp()),
    };

    let may_write = acl.allows(class_of(uid, gids, acl), Right::Write);
    let target_ok = match target {
        None => true,
        Some(gid) => gids.contains(&gid),
    };

    if may_write && target_ok {
        Ok(())
    } else {
        Err(denied_chgrp())
    }
}

fn denied(right: Right) -> AdbError {
    AdbError::PermissionDenied(format!("{right:?} is not permitted here"))
}

fn denied_chgrp() -> AdbError {
    AdbError::PermissionDenied(String::from(
        "changing the group requires write access and membership of the target group",
    ))
}
