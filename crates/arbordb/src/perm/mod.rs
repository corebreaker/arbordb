//! The `permissions` feature: user/password authentication and per-value ACLs
//! over a *protected* database.
//!
//! A protected database records `permissions` in its `required_features`, so a
//! binary built without this feature refuses to open it (see
//! [`AdbError::DatabaseProtected`](crate::AdbError::DatabaseProtected)). Users,
//! groups, and the key-wrapping keyring live as blobs in the reserved `$metadata`
//! table; per-value ACLs live in the `$inodes` table alongside the timestamps.
//!
//! A database is protected only via [`ArborDb::change_password`](crate::ArborDb::change_password)
//! on a non-protected handle, which promotes it and authenticates the caller as
//! the master user. Creation itself never produces a protected database.

mod access;
mod crypto;
mod integrity;
mod principal;
mod store;

pub(crate) use self::access::{authorize, authorize_chgrp, authorize_chown};
pub(crate) use self::integrity::{ct_eq, mac_value};
pub(crate) use self::principal::Principal;
pub(crate) use self::store::{
    add_group,
    add_user,
    assign,
    authenticate,
    change_user_password,
    forget_user,
    gid_of,
    group_members,
    is_protected,
    list_groups,
    list_users,
    name_of_group,
    name_of_user,
    promote_to_master,
    remove_group,
    rename_group,
    rename_user,
    uid_of,
    unassign,
    user_groups,
};

/// The master user's fixed id — the only principal that bypasses ACLs.
pub(crate) const MASTER_UID: u32 = 0;

/// The guest user's fixed id — anonymous, frozen, read-only.
pub(crate) const GUEST_UID: u32 = 1;

/// The master group's fixed id — administers users and groups (no ACL bypass).
pub(crate) const MASTER_GID: u32 = 0;

/// The super group's fixed id — read-only listing of users and groups.
pub(crate) const SUPER_GID: u32 = 1;

/// The master user's fixed name.
pub(crate) const MASTER_USER: &str = "master";

/// The guest user's fixed name.
pub(crate) const GUEST_USER: &str = "guest";

/// The master group's fixed name.
pub(crate) const MASTER_GROUP: &str = "master";

/// The super group's fixed name.
pub(crate) const SUPER_GROUP: &str = "super";
