//! The identity a database handle acts as.

use super::crypto::KEY_LEN;

/// The identity a database handle acts as.
pub(crate) enum Principal {
    /// The `permissions` feature is compiled in, but this database has no
    /// permission system — every operation is unrestricted.
    Unrestricted,

    /// Anonymous, read-only access to a protected database.
    Guest,

    /// An authenticated user of a protected database.
    User(Session),
}

/// An authenticated session: who the caller is and the integrity key they unlocked.
pub(crate) struct Session {
    name: String,
    uid:  u32,
    gids: Vec<u32>,
    key:  [u8; KEY_LEN],
}

impl Session {
    /// Assembles a session from a decoded user record and its unlocked key.
    pub(crate) fn new(name: String, uid: u32, gids: Vec<u32>, key: [u8; KEY_LEN]) -> Self {
        Self {
            name,
            uid,
            gids,
            key,
        }
    }

    /// The user's name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The user's id.
    pub(crate) fn uid(&self) -> u32 {
        self.uid
    }

    /// The ids of the groups the user belongs to.
    pub(crate) fn gids(&self) -> &[u32] {
        &self.gids
    }

    /// The unlocked database integrity key.
    pub(crate) fn key(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    // The predicates below are consumed by ACL enforcement (a later permissions
    // increment); they are defined here with the rest of the identity.

    /// Whether this is the master user, which bypasses ACLs entirely.
    pub(crate) fn is_master(&self) -> bool {
        self.uid == super::MASTER_UID
    }

    /// Whether the user belongs to the master group, which administers users and
    /// groups (but does not bypass ACLs).
    pub(crate) fn in_master_group(&self) -> bool {
        self.gids.contains(&super::MASTER_GID)
    }

    /// Whether the user belongs to the super group, which may list users and
    /// groups read-only.
    pub(crate) fn in_super_group(&self) -> bool {
        self.gids.contains(&super::SUPER_GID)
    }
}
