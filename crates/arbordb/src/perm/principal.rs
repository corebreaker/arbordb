//! The identity a database handle acts as.

use super::constants::{MASTER_UID, MASTER_GID, SUPER_GID};
use crate::crypto::{Signer, KEY_LEN, PUBKEY_LEN, SEED_LEN};

/// The identity a database handle acts as.
pub(crate) enum Principal {
    /// The `permissions` feature is compiled in, but this database has no
    /// permission system — every operation is unrestricted.
    Unrestricted,

    /// Anonymous, read-only access to a protected database. Holds the public
    /// verification key so a guest read still checks each value's signature, even
    /// though the guest has no private key to seal (write) one.
    Guest {
        /// The database's public verification key, read in the clear at open.
        pubkey: [u8; PUBKEY_LEN],
    },

    /// An authenticated user of a protected database. Boxed because a [`Session`]
    /// (with its expanded signer) is far larger than the other variants.
    User(Box<Session>),
}

/// An authenticated session: who the caller is and the secrets they unlocked — the
/// integrity key `K` (keys the value MAC) and the Ed25519 signing seed (signs values).
pub(crate) struct Session {
    /// The authenticated user's name.
    name:      String,
    /// The user's id.
    uid:       u32,
    /// The ids of the groups the user belongs to.
    gids:      Vec<u32>,
    /// The database integrity key this session unlocked.
    key:       [u8; KEY_LEN],
    /// The database signing seed this session unlocked (kept for re-wrapping the
    /// secret bundle into another user's keyring).
    sign_seed: [u8; SEED_LEN],
    /// The expanded signer over `sign_seed`, so a protected write never re-derives
    /// the signing key from the seed.
    signer:    Signer,
}

impl Session {
    /// Assembles a session from a decoded user record and its unlocked secrets.
    pub(crate) fn new(name: String, uid: u32, gids: Vec<u32>, key: [u8; KEY_LEN], sign_seed: [u8; SEED_LEN]) -> Self {
        Self {
            name,
            uid,
            gids,
            key,
            sign_seed,
            signer: Signer::new(&sign_seed),
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

    /// The unlocked database integrity key (keys the value MAC).
    pub(crate) fn key(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// The unlocked database signing seed (for re-wrapping into another keyring).
    pub(crate) fn sign_seed(&self) -> &[u8; SEED_LEN] {
        &self.sign_seed
    }

    /// The session's reusable value signer (keeps the expanded signing key).
    pub(crate) fn signer(&self) -> &Signer {
        &self.signer
    }

    // The predicates below are consumed by ACL enforcement (a later permissions
    // increment); they are defined here with the rest of the identity.

    /// Whether this is the master user, which bypasses ACLs entirely.
    pub(crate) fn is_master(&self) -> bool {
        self.uid == MASTER_UID
    }

    /// Whether the user belongs to the master group, which administers users and
    /// groups (but does not bypass ACLs).
    pub(crate) fn in_master_group(&self) -> bool {
        self.gids.contains(&MASTER_GID)
    }

    /// Whether the user belongs to the super group, which may list users and
    /// groups read-only.
    pub(crate) fn in_super_group(&self) -> bool {
        self.gids.contains(&SUPER_GID)
    }
}
