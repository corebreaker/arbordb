/// The master user's fixed id — the only principal that bypasses ACLs.
pub(crate) const MASTER_UID: u32 = 0;

/// The guest user's fixed id — anonymous, frozen, read-only.
pub(crate) const GUEST_UID: u32 = 1;

/// The master group's fixed id — administers users and groups (no ACL bypass).
pub(crate) const MASTER_GID: u32 = 0;

/// The super group's fixed id — read-only listing of users and groups.
pub(crate) const SUPER_GID: u32 = 1;

/// The master user's fixed name.
pub const MASTER_USER: &str = "master";

/// The guest user's fixed name.
pub const GUEST_USER: &str = "guest";

/// The master group's fixed name.
pub const MASTER_GROUP: &str = "master";

/// The super group's fixed name.
pub const SUPER_GROUP: &str = "super";
