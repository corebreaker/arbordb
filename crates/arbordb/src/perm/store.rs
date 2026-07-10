//! The users, groups, and key-wrapping keyring of a protected database, persisted
//! as two blobs (`users`, `groups`) in the reserved `$metadata` table.
//!
//! A user record optionally carries a *keyring entry* — the database integrity
//! key `K` wrapped under that user's password. Authenticating unwraps `K`; a user
//! with no keyring entry (the guest) simply cannot authenticate.

use super::{
    constants::{GUEST_UID, GUEST_USER, MASTER_GID, MASTER_GROUP, MASTER_UID, MASTER_USER, SUPER_GID, SUPER_GROUP},
    principal::Session,
    integrity,
};

use crate::{
    crypto::{self, KEY_LEN, NONCE_LEN, SALT_LEN},
    codec::{put_bytes, put_u32, Reader},
    constants::{META_CONTROL_MAC_KEY, META_EPOCH_KEY, META_GROUPS_KEY, META_USERS_KEY},
    engine::{data_def, required_features, require_feature, META_TABLE},
    error::{AdbError, AdbResult},
    inode::{seal_mac, set_default_acl, INODES_TABLE},
    AKey,
};

use redb::{Database, ReadableDatabase, ReadableTable, Table, TableHandle};

/// The write handle to the metadata table.
type MetaTable<'txn> = Table<'txn, &'static str, &'static [u8]>;

/// The integrity key `K` wrapped under one user's password.
struct KeyringEntry {
    /// The per-user salt fed to the password key-derivation function.
    salt:    [u8; SALT_LEN],
    /// The AEAD nonce used to wrap the integrity key.
    nonce:   [u8; NONCE_LEN],
    /// The integrity key `K`, encrypted under the password-derived key.
    wrapped: Vec<u8>,
}

/// A stored user: identity, group memberships, frozen flag, and (unless the user
/// has no password) its wrapped copy of the integrity key.
struct UserRecord {
    /// The user's name.
    name:    String,
    /// The user's id.
    uid:     u32,
    /// Whether the user is frozen (the guest): kept, but unable to authenticate or
    /// be modified.
    frozen:  bool,
    /// The ids of the groups the user belongs to.
    gids:    Vec<u32>,
    /// The user's wrapped integrity key, or `None` for a passwordless user.
    keyring: Option<KeyringEntry>,
}

/// Every user plus the next id to allocate.
struct Users {
    /// The next user id to allocate.
    next_uid: u32,
    /// Every stored user.
    users:    Vec<UserRecord>,
}

impl Users {
    fn load<R: ReadableTable<&'static str, &'static [u8]>>(meta: &R) -> AdbResult<Self> {
        match meta.get(META_USERS_KEY)? {
            Some(guard) => Self::decode(guard.value()),
            None => Ok(Self {
                next_uid: 2,
                users:    Vec::new(),
            }),
        }
    }

    fn decode(bytes: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(bytes);
        let next_uid = r.u32()?;
        let count = r.u32()?;

        let mut users = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = utf8(r.bytes()?)?;
            let uid = r.u32()?;
            let frozen = r.u8()? != 0;

            let gid_count = r.u32()?;
            let mut gids = Vec::with_capacity(gid_count as usize);
            for _ in 0..gid_count {
                gids.push(r.u32()?);
            }

            let keyring = match r.u8()? {
                0 => None,
                _ => Some(KeyringEntry {
                    salt:    r.array::<SALT_LEN>()?,
                    nonce:   r.array::<NONCE_LEN>()?,
                    wrapped: r.bytes()?.to_vec(),
                }),
            };

            users.push(UserRecord {
                name,
                uid,
                frozen,
                gids,
                keyring,
            });
        }

        Ok(Self {
            next_uid,
            users,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        put_u32(&mut buf, self.next_uid);
        put_u32(&mut buf, self.users.len() as u32);

        for user in &self.users {
            put_bytes(&mut buf, user.name.as_bytes());
            put_u32(&mut buf, user.uid);
            buf.push(user.frozen as u8);

            put_u32(&mut buf, user.gids.len() as u32);
            for gid in &user.gids {
                put_u32(&mut buf, *gid);
            }

            match &user.keyring {
                None => buf.push(0),
                Some(entry) => {
                    buf.push(1);
                    buf.extend_from_slice(&entry.salt);
                    buf.extend_from_slice(&entry.nonce);
                    put_bytes(&mut buf, &entry.wrapped);
                }
            }
        }

        buf
    }

    fn store(&self, meta: &mut MetaTable<'_>) -> AdbResult<()> {
        meta.insert(META_USERS_KEY, self.encode().as_slice())?;

        Ok(())
    }
}

/// A stored group: a name and its id.
struct GroupRecord {
    /// The group's name.
    name: String,
    /// The group's id.
    gid:  u32,
}

/// Every group plus the next id to allocate.
struct Groups {
    /// The next group id to allocate.
    next_gid: u32,
    /// Every stored group.
    groups:   Vec<GroupRecord>,
}

impl Groups {
    fn load<R: ReadableTable<&'static str, &'static [u8]>>(meta: &R) -> AdbResult<Self> {
        match meta.get(META_GROUPS_KEY)? {
            Some(guard) => Self::decode(guard.value()),
            None => Ok(Self {
                next_gid: 2,
                groups:   Vec::new(),
            }),
        }
    }

    fn decode(bytes: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(bytes);
        let next_gid = r.u32()?;
        let count = r.u32()?;

        let mut groups = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = utf8(r.bytes()?)?;
            let gid = r.u32()?;
            groups.push(GroupRecord {
                name,
                gid,
            });
        }

        Ok(Self {
            next_gid,
            groups,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        put_u32(&mut buf, self.next_gid);
        put_u32(&mut buf, self.groups.len() as u32);

        for group in &self.groups {
            put_bytes(&mut buf, group.name.as_bytes());
            put_u32(&mut buf, group.gid);
        }

        buf
    }

    fn store(&self, meta: &mut MetaTable<'_>) -> AdbResult<()> {
        meta.insert(META_GROUPS_KEY, self.encode().as_slice())?;

        Ok(())
    }
}

/// Decodes a UTF-8 string, mapping invalid bytes to a corruption error.
fn utf8(bytes: &[u8]) -> AdbResult<String> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| AdbError::Corrupt("invalid utf-8 in the permission store".into()))
}

/// The current control-plane epoch (0 when the database has never sealed one).
fn read_epoch<R: ReadableTable<&'static str, &'static [u8]>>(meta: &R) -> AdbResult<u64> {
    match meta.get(META_EPOCH_KEY)? {
        Some(guard) => {
            let bytes: [u8; 8] = guard
                .value()
                .try_into()
                .map_err(|_| AdbError::Corrupt("the control epoch has the wrong length".into()))?;

            Ok(u64::from_be_bytes(bytes))
        }
        None => Ok(0),
    }
}

/// Re-seals the control plane after a change: bumps the monotonic epoch and rewrites
/// the MAC binding it to the current user and group blobs. Every mutation of the
/// store calls this so the tag always matches what is stored.
fn seal_control(meta: &mut MetaTable<'_>, key: &[u8]) -> AdbResult<()> {
    let users = meta
        .get(META_USERS_KEY)?
        .map(|guard| guard.value().to_vec())
        .unwrap_or_default();
    let groups = meta
        .get(META_GROUPS_KEY)?
        .map(|guard| guard.value().to_vec())
        .unwrap_or_default();
    let epoch = read_epoch(&*meta)? + 1;

    meta.insert(META_EPOCH_KEY, epoch.to_be_bytes().as_slice())?;
    meta.insert(
        META_CONTROL_MAC_KEY,
        integrity::mac_control(key, epoch, &users, &groups).as_slice(),
    )?;

    Ok(())
}

/// Verifies the control plane against `key`. A mismatch means the user/group store
/// was altered outside the library — for example by a program that opened the redb
/// file directly — and surfaces as [`AdbError::Tampered`].
fn verify_control<R: ReadableTable<&'static str, &'static [u8]>>(meta: &R, key: &[u8]) -> AdbResult<()> {
    let users = meta
        .get(META_USERS_KEY)?
        .map(|guard| guard.value().to_vec())
        .unwrap_or_default();
    let groups = meta
        .get(META_GROUPS_KEY)?
        .map(|guard| guard.value().to_vec())
        .unwrap_or_default();
    let epoch = read_epoch(meta)?;
    let expected = integrity::mac_control(key, epoch, &users, &groups);

    match meta.get(META_CONTROL_MAC_KEY)? {
        Some(guard) if integrity::ct_eq(guard.value(), &expected) => Ok(()),
        _ => Err(AdbError::Tampered(String::from(
            "the permission store failed its integrity check",
        ))),
    }
}

/// Whether `db` has a permission system (it records `permissions` as required).
pub(crate) fn is_protected(db: &Database) -> AdbResult<bool> {
    let txn = db.begin_read()?;
    let meta = txn.open_table(META_TABLE)?;

    Ok(required_features(&meta)?.iter().any(|feature| feature == "permissions"))
}

/// Authenticates `user`/`password` against the stored keyring, returning the
/// session (which holds the unwrapped integrity key). Unknown user, no password,
/// or wrong password all surface as [`AuthenticationFailed`](AdbError::AuthenticationFailed).
pub(crate) fn authenticate(db: &Database, user: &str, password: &str) -> AdbResult<Session> {
    let txn = db.begin_read()?;
    let meta = txn.open_table(META_TABLE)?;
    let users = Users::load(&meta)?;

    let record = users
        .users
        .iter()
        .find(|candidate| candidate.name == user)
        .ok_or(AdbError::AuthenticationFailed)?;

    let keyring = record.keyring.as_ref().ok_or(AdbError::AuthenticationFailed)?;

    let key = crypto::unwrap_key(password, &keyring.salt, &keyring.nonce, &keyring.wrapped)?;
    let key: [u8; KEY_LEN] = key
        .as_slice()
        .try_into()
        .map_err(|_| AdbError::Corrupt("the wrapped integrity key has the wrong length".into()))?;

    // With `K` in hand, confirm the user/group store has not been altered under us.
    verify_control(&meta, &key)?;

    Ok(Session::new(record.name.clone(), record.uid, record.gids.clone(), key))
}

/// Promotes a non-protected `db` to a protected one: mints a fresh integrity key,
/// creates the master and guest users and the master and super groups, records
/// `permissions` as required, and returns the master session.
pub(crate) fn promote_to_master(db: &Database, password: &str) -> AdbResult<Session> {
    let key = crypto::random_key()?;
    let (salt, nonce, wrapped) = crypto::wrap_key(password, &key)?;

    // Snapshot the existing user tables so their vnodes can be back-filled with a
    // default ACL and an integrity tag below (data written before protection).
    let tables: Vec<String> = {
        let read = db.begin_read()?;
        read.list_tables()?
            .map(|handle| handle.name().to_string())
            .filter(|name| !crate::constants::is_reserved_table_name(name))
            .collect()
    };

    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;

        if meta.get(META_USERS_KEY)?.is_some() {
            return Err(AdbError::CannotAccess("the database is already protected".into()));
        }

        let users = Users {
            next_uid: 2,
            users:    vec![
                UserRecord {
                    name:    MASTER_USER.to_string(),
                    uid:     MASTER_UID,
                    frozen:  false,
                    gids:    vec![MASTER_GID],
                    keyring: Some(KeyringEntry {
                        salt,
                        nonce,
                        wrapped,
                    }),
                },
                UserRecord {
                    name:    GUEST_USER.to_string(),
                    uid:     GUEST_UID,
                    frozen:  true,
                    gids:    Vec::new(),
                    keyring: None,
                },
            ],
        };

        let groups = Groups {
            next_gid: 2,
            groups:   vec![
                GroupRecord {
                    name: MASTER_GROUP.to_string(),
                    gid:  MASTER_GID,
                },
                GroupRecord {
                    name: SUPER_GROUP.to_string(),
                    gid:  SUPER_GID,
                },
            ],
        };

        users.store(&mut meta)?;
        groups.store(&mut meta)?;
        require_feature(&mut meta, "permissions")?;
        seal_control(&mut meta, &key)?;
    }

    // Back-fill: stamp a master-owned default ACL and seal an integrity tag on every
    // pre-existing vnode, so authenticated reads verify and non-root data has an owner.
    {
        let mut inodes = txn.open_table(INODES_TABLE)?;
        for table in &tables {
            let rows: Vec<(AKey, Vec<u8>)> = {
                let data = txn.open_table(data_def(table))?;
                data.iter()?
                    .map(|row| {
                        let (akey, entry) = row?;

                        Ok((AKey::from(akey.value()), entry.value().to_vec()))
                    })
                    .collect::<AdbResult<_>>()?
            };

            for (akey, entry) in rows {
                if akey != AKey::ROOT {
                    set_default_acl(&mut inodes, table, akey, MASTER_UID)?;
                }

                let acl = crate::inode::read_acl(&inodes, table, akey)?
                    .map(|acl| acl.encode())
                    .unwrap_or_default();

                seal_mac(
                    &mut inodes,
                    table,
                    akey,
                    integrity::mac_value(&key, table, akey, &entry, &acl),
                )?;
            }
        }
    }

    txn.commit()?;

    Ok(Session::new(MASTER_USER.to_string(), MASTER_UID, vec![MASTER_GID], key))
}

/// Re-wraps the current session's integrity key under a new password, replacing
/// that user's keyring entry.
pub(crate) fn change_user_password(db: &Database, session: &Session, new_password: &str) -> AdbResult<()> {
    let (salt, nonce, wrapped) = crypto::wrap_key(new_password, session.key())?;

    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut users = Users::load(&meta)?;

        let record = users
            .users
            .iter_mut()
            .find(|candidate| candidate.uid == session.uid())
            .ok_or_else(|| AdbError::Corrupt("the authenticated user is missing from the store".into()))?;

        record.keyring = Some(KeyringEntry {
            salt,
            nonce,
            wrapped,
        });

        users.store(&mut meta)?;
        seal_control(&mut meta, session.key())?;
    }
    txn.commit()?;

    Ok(())
}

/// Whether `uid` is a built-in user (master or guest) that cannot be removed or renamed.
fn is_protected_user(uid: u32) -> bool {
    uid == MASTER_UID || uid == GUEST_UID
}

/// Whether `gid` is a built-in group (master or super) that cannot be removed or renamed.
fn is_protected_group(gid: u32) -> bool {
    gid == MASTER_GID || gid == SUPER_GID
}

/// Adds a user `name` with `password`, wrapping the caller's integrity `key` under
/// it. When `create_group`, also creates a same-named group and puts the user in it.
pub(crate) fn add_user(db: &Database, key: &[u8], name: &str, password: &str, create_group: bool) -> AdbResult<()> {
    let (salt, nonce, wrapped) = crypto::wrap_key(password, key)?;

    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut users = Users::load(&meta)?;

        if users.users.iter().any(|candidate| candidate.name == name) {
            return Err(AdbError::CannotAccess(format!("a user named '{name}' already exists")));
        }

        let uid = users.next_uid;
        users.next_uid += 1;

        let mut gids = Vec::new();
        if create_group {
            let mut groups = Groups::load(&meta)?;
            if groups.groups.iter().any(|group| group.name == name) {
                return Err(AdbError::CannotAccess(format!("a group named '{name}' already exists")));
            }

            let gid = groups.next_gid;
            groups.next_gid += 1;
            groups.groups.push(GroupRecord {
                name: name.to_string(),
                gid,
            });
            gids.push(gid);
            groups.store(&mut meta)?;
        }

        users.users.push(UserRecord {
            name: name.to_string(),
            uid,
            frozen: false,
            gids,
            keyring: Some(KeyringEntry {
                salt,
                nonce,
                wrapped,
            }),
        });
        users.store(&mut meta)?;
        seal_control(&mut meta, key)?;
    }
    txn.commit()?;

    Ok(())
}

/// Adds an empty group `name`.
pub(crate) fn add_group(db: &Database, key: &[u8], name: &str) -> AdbResult<()> {
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut groups = Groups::load(&meta)?;

        if groups.groups.iter().any(|group| group.name == name) {
            return Err(AdbError::CannotAccess(format!("a group named '{name}' already exists")));
        }

        let gid = groups.next_gid;
        groups.next_gid += 1;
        groups.groups.push(GroupRecord {
            name: name.to_string(),
            gid,
        });
        groups.store(&mut meta)?;
        seal_control(&mut meta, key)?;
    }
    txn.commit()?;

    Ok(())
}

/// Renames user `old` to `new`. The built-in master and guest users cannot be renamed.
pub(crate) fn rename_user(db: &Database, key: &[u8], old: &str, new: &str) -> AdbResult<()> {
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut users = Users::load(&meta)?;

        if users.users.iter().any(|candidate| candidate.name == new) {
            return Err(AdbError::CannotAccess(format!("a user named '{new}' already exists")));
        }

        let record = users
            .users
            .iter_mut()
            .find(|candidate| candidate.name == old)
            .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{old}'")))?;

        if is_protected_user(record.uid) {
            return Err(AdbError::PermissionDenied(format!(
                "the built-in user '{old}' cannot be renamed"
            )));
        }

        record.name = new.to_string();
        users.store(&mut meta)?;
        seal_control(&mut meta, key)?;
    }
    txn.commit()?;

    Ok(())
}

/// Renames group `old` to `new`. The built-in master and super groups cannot be renamed.
pub(crate) fn rename_group(db: &Database, key: &[u8], old: &str, new: &str) -> AdbResult<()> {
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut groups = Groups::load(&meta)?;

        if groups.groups.iter().any(|group| group.name == new) {
            return Err(AdbError::CannotAccess(format!("a group named '{new}' already exists")));
        }

        let record = groups
            .groups
            .iter_mut()
            .find(|group| group.name == old)
            .ok_or_else(|| AdbError::CannotAccess(format!("no group named '{old}'")))?;

        if is_protected_group(record.gid) {
            return Err(AdbError::PermissionDenied(format!(
                "the built-in group '{old}' cannot be renamed"
            )));
        }

        record.name = new.to_string();
        groups.store(&mut meta)?;
        seal_control(&mut meta, key)?;
    }
    txn.commit()?;

    Ok(())
}

/// Adds `user` to `group` (idempotent). A frozen user (the guest) cannot be assigned.
pub(crate) fn assign(db: &Database, key: &[u8], user: &str, group: &str) -> AdbResult<()> {
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut users = Users::load(&meta)?;
        let groups = Groups::load(&meta)?;

        let gid = groups
            .groups
            .iter()
            .find(|candidate| candidate.name == group)
            .map(|candidate| candidate.gid)
            .ok_or_else(|| AdbError::CannotAccess(format!("no group named '{group}'")))?;

        let record = users
            .users
            .iter_mut()
            .find(|candidate| candidate.name == user)
            .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{user}'")))?;

        if record.frozen {
            return Err(AdbError::Frozen(user.to_string()));
        }

        if !record.gids.contains(&gid) {
            record.gids.push(gid);
            users.store(&mut meta)?;
            seal_control(&mut meta, key)?;
        }
    }
    txn.commit()?;

    Ok(())
}

/// Removes `user` from `group` (idempotent).
pub(crate) fn unassign(db: &Database, key: &[u8], user: &str, group: &str) -> AdbResult<()> {
    let txn = db.begin_write()?;
    {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut users = Users::load(&meta)?;
        let groups = Groups::load(&meta)?;

        let gid = groups
            .groups
            .iter()
            .find(|candidate| candidate.name == group)
            .map(|candidate| candidate.gid)
            .ok_or_else(|| AdbError::CannotAccess(format!("no group named '{group}'")))?;

        let record = users
            .users
            .iter_mut()
            .find(|candidate| candidate.name == user)
            .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{user}'")))?;

        if record.gids.contains(&gid) {
            record.gids.retain(|candidate| *candidate != gid);
            users.store(&mut meta)?;
            seal_control(&mut meta, key)?;
        }
    }
    txn.commit()?;

    Ok(())
}

/// The names of every user, sorted.
pub(crate) fn list_users(db: &Database) -> AdbResult<Vec<String>> {
    let txn = db.begin_read()?;
    let meta = txn.open_table(META_TABLE)?;

    let mut names: Vec<String> = Users::load(&meta)?.users.into_iter().map(|user| user.name).collect();
    names.sort();

    Ok(names)
}

/// The names of every group, sorted.
pub(crate) fn list_groups(db: &Database) -> AdbResult<Vec<String>> {
    let txn = db.begin_read()?;
    let meta = txn.open_table(META_TABLE)?;

    let mut names: Vec<String> = Groups::load(&meta)?
        .groups
        .into_iter()
        .map(|group| group.name)
        .collect();
    names.sort();

    Ok(names)
}

/// The names of the groups `user` belongs to, sorted.
pub(crate) fn user_groups(db: &Database, user: &str) -> AdbResult<Vec<String>> {
    let txn = db.begin_read()?;
    let meta = txn.open_table(META_TABLE)?;
    let users = Users::load(&meta)?;
    let groups = Groups::load(&meta)?;

    let record = users
        .users
        .iter()
        .find(|candidate| candidate.name == user)
        .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{user}'")))?;

    let mut names: Vec<String> = groups
        .groups
        .iter()
        .filter(|group| record.gids.contains(&group.gid))
        .map(|group| group.name.clone())
        .collect();
    names.sort();

    Ok(names)
}

/// The uid of the user named `name`, if any (resolved within an open transaction).
pub(crate) fn uid_of<R: ReadableTable<&'static str, &'static [u8]>>(meta: &R, name: &str) -> AdbResult<Option<u32>> {
    Ok(Users::load(meta)?
        .users
        .into_iter()
        .find(|user| user.name == name)
        .map(|user| user.uid))
}

/// The gid of the group named `name`, if any (resolved within an open transaction).
pub(crate) fn gid_of<R: ReadableTable<&'static str, &'static [u8]>>(meta: &R, name: &str) -> AdbResult<Option<u32>> {
    Ok(Groups::load(meta)?
        .groups
        .into_iter()
        .find(|group| group.name == name)
        .map(|group| group.gid))
}

/// The name of the user with `uid`, if any.
pub(crate) fn name_of_user<R: ReadableTable<&'static str, &'static [u8]>>(
    meta: &R,
    uid: u32,
) -> AdbResult<Option<String>> {
    Ok(Users::load(meta)?
        .users
        .into_iter()
        .find(|user| user.uid == uid)
        .map(|user| user.name))
}

/// The name of the group with `gid`, if any.
pub(crate) fn name_of_group<R: ReadableTable<&'static str, &'static [u8]>>(
    meta: &R,
    gid: u32,
) -> AdbResult<Option<String>> {
    Ok(Groups::load(meta)?
        .groups
        .into_iter()
        .find(|group| group.gid == gid)
        .map(|group| group.name))
}

/// The names of the users belonging to `group`, sorted.
pub(crate) fn group_members(db: &Database, group: &str) -> AdbResult<Vec<String>> {
    let txn = db.begin_read()?;
    let meta = txn.open_table(META_TABLE)?;
    let users = Users::load(&meta)?;
    let groups = Groups::load(&meta)?;

    let gid = groups
        .groups
        .iter()
        .find(|candidate| candidate.name == group)
        .map(|candidate| candidate.gid)
        .ok_or_else(|| AdbError::CannotAccess(format!("no group named '{group}'")))?;

    let mut names: Vec<String> = users
        .users
        .iter()
        .filter(|user| user.gids.contains(&gid))
        .map(|user| user.name.clone())
        .collect();
    names.sort();

    Ok(names)
}

/// Removes group `name`: unassigns it from every user and strips its gid from every
/// vnode ACL. The built-in master and super groups cannot be removed.
pub(crate) fn remove_group(db: &Database, key: &[u8], name: &str) -> AdbResult<()> {
    let txn = db.begin_write()?;

    let gid = {
        let mut meta = txn.open_table(META_TABLE)?;
        let mut groups = Groups::load(&meta)?;

        let gid = groups
            .groups
            .iter()
            .find(|group| group.name == name)
            .map(|group| group.gid)
            .ok_or_else(|| AdbError::CannotAccess(format!("no group named '{name}'")))?;

        if is_protected_group(gid) {
            return Err(AdbError::PermissionDenied(format!(
                "the built-in group '{name}' cannot be removed"
            )));
        }

        let mut users = Users::load(&meta)?;
        for user in &mut users.users {
            user.gids.retain(|candidate| *candidate != gid);
        }
        users.store(&mut meta)?;

        groups.groups.retain(|group| group.gid != gid);
        groups.store(&mut meta)?;
        seal_control(&mut meta, key)?;

        gid
    };

    {
        let mut inodes = txn.open_table(crate::inode::INODES_TABLE)?;
        crate::inode::strip_group(&mut inodes, gid)?;
    }

    txn.commit()?;

    Ok(())
}

/// Removes the user record with `uid` (the caller reaps its owned values first).
pub(crate) fn forget_user(meta: &mut MetaTable<'_>, key: &[u8], uid: u32) -> AdbResult<()> {
    let mut users = Users::load(meta)?;
    users.users.retain(|user| user.uid != uid);
    users.store(meta)?;
    seal_control(meta, key)?;

    Ok(())
}
