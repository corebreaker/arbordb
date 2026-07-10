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
mod integrity;
mod principal;
mod store;

pub mod constants;

pub(crate) use self::{
    access::{authorize, authorize_chgrp, authorize_chown},
    integrity::{ct_eq, mac_value},
    principal::Principal,
    store::{
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
    },
};
