//! The value-access authorizer: may a principal exercise a graded right on a vnode?
//!
//! - The **master user** and an **unrestricted** (non-protected) handle bypass ACLs.
//! - The **guest** is strictly read-only: it may only [`Access`](Rights::Access) (read/traverse), never `Modify` or
//!   `Delete`, whatever the ACL says.
//! - The **root** vnode is special — everyone may access it, any non-guest may create children in it, and its ACL is
//!   immutable (never stored).
//! - A vnode with **no ACL** (e.g. one predating protection) is reachable only by the master user or a **master-group**
//!   member.
//! - Otherwise the caller's effective grade (owner, then the strongest of the groups it shares with the vnode, then
//!   other) must include the grade the operation needs. Membership of the master or super *group* grants no bypass
//!   here.

use super::{constants::GUEST_UID, Principal};
use crate::{
    acl::Rights,
    error::{AdbError, AdbResult},
    inode::Acl,
    AKey,
};

/// Checks that `principal` may exercise the `needed` grade on vnode `akey` governed
/// by `acl` (`None` when the vnode has no ACL yet).
pub(crate) fn authorize(principal: &Principal, akey: AKey, acl: Option<&Acl>, needed: Rights) -> AdbResult<()> {
    // The guest is read-only, whatever the ACL or the vnode says: it may only access.
    if matches!(principal, Principal::Guest { .. }) && needed > Rights::Access {
        return Err(denied(needed));
    }

    // The root is special: access for everyone, create-children for any non-guest
    // (already screened above), and no ACL to consult.
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
                Err(denied(needed))
            }
        }
        Some(acl) => {
            let (uid, gids): (u32, &[u32]) = match principal {
                Principal::User(session) => (session.uid(), session.gids()),
                Principal::Guest {
                    ..
                } => (GUEST_UID, &[]),
                // Unrestricted and the master user are handled above.
                Principal::Unrestricted => return Ok(()),
            };

            if acl.effective_rights(uid, gids).includes(needed) {
                Ok(())
            } else {
                Err(denied(needed))
            }
        }
    }
}

/// Checks that `principal` may change a vnode's **owner**. The `Modify` grade alone
/// is not enough — only the master user, a master-group member, or the current owner
/// may chown.
pub(crate) fn authorize_chown(principal: &Principal, acl: &Acl) -> AdbResult<()> {
    let allowed = match principal {
        Principal::Unrestricted => true,
        Principal::User(session) if session.is_master() || session.in_master_group() => true,
        Principal::User(session) => session.uid() == acl.owner_uid(),
        Principal::Guest {
            ..
        } => false,
    };

    if allowed {
        Ok(())
    } else {
        Err(AdbError::PermissionDenied(String::from(
            "changing the owner requires ownership or administration",
        )))
    }
}

fn denied(needed: Rights) -> AdbError {
    AdbError::PermissionDenied(format!("{needed:?} access is not permitted here"))
}
