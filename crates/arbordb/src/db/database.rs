//! The database handle and the database-wide shared state behind it.

use super::DbInner;
use crate::{
    constants::is_reserved_table_name,
    engine,
    error::{AdbError, AdbResult},
    table::Table,
};

#[cfg(feature = "permissions")]
use crate::perm::{self, Principal};

use redb::{backends::InMemoryBackend, Database, ReadableDatabase, TableHandle};
use std::{
    collections::HashMap,
    path::Path,
    sync::{atomic::AtomicU64, Arc, Mutex, RwLock},
};

/// An ArborDb database: one file (or an in-memory store) holding any number of
/// named [`Table`]s.
#[derive(Clone)]
pub struct ArborDb {
    inner: Arc<DbInner>,

    /// The identity this handle acts as. Re-authentication mints a new handle with
    /// a different principal; clones share it.
    #[cfg(feature = "permissions")]
    principal: Arc<Principal>,
}

impl ArborDb {
    /// Creates a new database file at `path` (truncating any existing one).
    pub fn create(path: impl AsRef<Path>) -> AdbResult<Self> {
        Self::wrap(Database::create(path)?)
    }

    /// Opens an existing database file at `path`.
    pub fn open(path: impl AsRef<Path>) -> AdbResult<Self> {
        Self::wrap(Database::open(path)?)
    }

    /// Creates a database kept entirely in memory (useful for tests).
    pub fn create_in_memory() -> AdbResult<Self> {
        Self::wrap(Database::builder().create_with_backend(InMemoryBackend::new())?)
    }

    pub fn is_protected(&self) -> AdbResult<bool> {
        #[cfg(feature = "permissions")]
        if cfg!(feature = "permissions") {
            return perm::is_protected(self.inner.db());
        }

        Ok(false)
    }

    /// Bootstraps the metadata table then wraps the engine handle. A protected
    /// database opens as the guest principal; an unprotected one is unrestricted.
    fn wrap(db: Database) -> AdbResult<Self> {
        engine::bootstrap_metadata(&db)?;

        #[cfg(feature = "permissions")]
        let principal = if perm::is_protected(&db)? {
            Arc::new(Principal::Guest)
        } else {
            Arc::new(Principal::Unrestricted)
        };

        Ok(Self {
            inner: Arc::new(DbInner::new(
                db,
                AtomicU64::new(0),
                RwLock::new(()),
                Mutex::new(HashMap::new()),
            )),
            #[cfg(feature = "permissions")]
            principal,
        })
    }

    /// Opens a handle to the table named `name`, from which transactions start.
    /// The table is created lazily on its first write. The empty name is rejected,
    /// as is any name starting with `$` (reserved for engine-internal tables such
    /// as `$metadata`, `$index`, and `$inodes`).
    pub fn open_table(&self, name: &str) -> AdbResult<Table> {
        if name.is_empty() {
            return Err(AdbError::InvalidTableName(String::from("a table name cannot be empty")));
        }

        if is_reserved_table_name(name) {
            return Err(AdbError::InvalidTableName(format!(
                "'{name}' is reserved (a table name cannot start with '$')"
            )));
        }

        let cache = self.inner.cache(name)?;

        Ok(Table::new(
            Arc::clone(&self.inner),
            name.to_string(),
            cache,
            #[cfg(feature = "permissions")]
            Arc::clone(&self.principal),
        ))
    }

    /// The names of every user table, in sorted order.
    ///
    /// Reserved (`$`-prefixed) tables are omitted. A table is created lazily on
    /// its first write, so one that was [`open_table`](Self::open_table)ed but
    /// never written to does not appear yet.
    pub fn list_tables(&self) -> AdbResult<Vec<String>> {
        let txn = self.inner.db().begin_read()?;

        // Collect owned names before the transaction is dropped; the reserved
        // (`$`-prefixed) tables are engine bookkeeping, not user tables.
        let mut names: Vec<String> = txn
            .list_tables()?
            .map(|handle| handle.name().to_string())
            .filter(|name| !is_reserved_table_name(name))
            .collect();

        names.sort();

        Ok(names)
    }

    /// Persists buffered vnode access times to the `$inodes` table.
    ///
    /// Reads record a vnode's access time in memory; it is otherwise written only on
    /// the next committed write. Call this after a read-only burst to make the
    /// access times durable. A no-op when nothing is buffered.
    #[cfg(feature = "entry-timestamps")]
    pub fn flush_access_times(&self) -> AdbResult<()> {
        let batch = self.inner.drain_access_log();
        if batch.is_empty() {
            return Ok(());
        }

        let txn = self.inner.db().begin_write()?;
        {
            let mut inodes = txn.open_table(crate::inode::INODES_TABLE)?;
            for ((table, akey), when) in batch {
                crate::inode::bump_access(&mut inodes, &table, akey, when)?;
            }
        }

        let guard = self
            .inner
            .version_lock()
            .write()
            .map_err(|_| AdbError::CannotAccess(String::from("the version lock was poisoned")))?;

        txn.commit()?;
        self.inner.bump_generation();
        drop(guard);

        Ok(())
    }
}

#[cfg(feature = "permissions")]
impl ArborDb {
    /// Opens the database at `path` and authenticates as `user`.
    ///
    /// Errors with [`NoPermissions`](AdbError::NoPermissions) if the database has
    /// no permission system, or [`AuthenticationFailed`](AdbError::AuthenticationFailed)
    /// for an unknown user or a wrong password.
    pub fn open_with_authentication(path: impl AsRef<Path>, user: &str, password: &str) -> AdbResult<Self> {
        Self::open(path)?.with_authentication(user, password)
    }

    /// Re-authenticates this handle as `user`, returning a handle that acts as that
    /// user. Errors with [`NoPermissions`](AdbError::NoPermissions) on a database
    /// that has no permission system.
    pub fn with_authentication(self, user: &str, password: &str) -> AdbResult<Self> {
        if matches!(self.principal.as_ref(), Principal::Unrestricted) {
            return Err(AdbError::NoPermissions);
        }

        let session = perm::authenticate(self.inner.db(), user, password)?;

        Ok(Self {
            inner:     self.inner,
            principal: Arc::new(Principal::User(session)),
        })
    }

    /// Changes a password.
    ///
    /// On a database with **no** permission system this *promotes* it: the caller
    /// becomes the master user, `new_password` becomes the master password, and the
    /// database is henceforth protected. On a protected database it changes the
    /// **current** user's password. The guest user has no password and is rejected.
    pub fn change_password(self, new_password: &str) -> AdbResult<Self> {
        match self.principal.as_ref() {
            Principal::Guest => {
                return Err(AdbError::PermissionDenied(String::from(
                    "the guest user has no password to change",
                )));
            }
            Principal::Unrestricted => {
                let session = perm::promote_to_master(self.inner.db(), new_password)?;

                return Ok(Self {
                    inner:     Arc::clone(&self.inner),
                    principal: Arc::new(Principal::User(session)),
                });
            }
            Principal::User(session) => {
                perm::change_user_password(self.inner.db(), session, new_password)?;
            }
        }

        Ok(self)
    }

    /// The name of the user this handle acts as: the authenticated user, `"guest"`
    /// on an unauthenticated protected database, or `None` when the database has no
    /// permission system.
    pub fn current_user(&self) -> Option<&str> {
        match self.principal.as_ref() {
            Principal::Unrestricted => None,
            Principal::Guest => Some(crate::perm::GUEST_USER),
            Principal::User(session) => Some(session.name()),
        }
    }

    /// The integrity key of an administering session (the master user or a
    /// master-group member), or [`PermissionDenied`](AdbError::PermissionDenied).
    fn admin_key(&self) -> AdbResult<&[u8]> {
        match self.principal.as_ref() {
            Principal::User(session) if session.is_master() || session.in_master_group() => Ok(session.key()),
            _ => Err(AdbError::PermissionDenied(String::from(
                "administration requires the master user or a master-group member",
            ))),
        }
    }

    /// Requires a session that may *list* users and groups — an administrator or a
    /// super-group member.
    fn require_lister(&self) -> AdbResult<()> {
        match self.principal.as_ref() {
            Principal::User(session)
                if session.is_master() || session.in_master_group() || session.in_super_group() =>
            {
                Ok(())
            }
            _ => Err(AdbError::PermissionDenied(String::from(
                "listing users and groups requires an administrator or a super-group member",
            ))),
        }
    }

    /// Creates user `name` with `password`. When `create_group`, also creates a
    /// same-named group and adds the user to it. Administrators only.
    pub fn add_user(&self, name: &str, password: &str, create_group: bool) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::add_user(self.inner.db(), key, name, password, create_group)
    }

    /// Creates an empty group `name`. Administrators only.
    pub fn add_group(&self, name: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::add_group(self.inner.db(), key, name)
    }

    /// Renames a user. The built-in master and guest users cannot be renamed.
    /// Administrators only.
    pub fn rename_user(&self, old: &str, new: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::rename_user(self.inner.db(), key, old, new)
    }

    /// Renames a group. The built-in master and super groups cannot be renamed.
    /// Administrators only.
    pub fn rename_group(&self, old: &str, new: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::rename_group(self.inner.db(), key, old, new)
    }

    /// Adds `user` to `group`. The frozen guest user cannot be assigned to any group.
    /// Administrators only.
    pub fn assign_user_to_group(&self, user: &str, group: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::assign(self.inner.db(), key, user, group)
    }

    /// Removes `user` from `group`. Administrators only.
    pub fn remove_user_from_group(&self, user: &str, group: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::unassign(self.inner.db(), key, user, group)
    }

    /// The names of every user, sorted. Administrators and super-group members.
    pub fn list_users(&self) -> AdbResult<Vec<String>> {
        self.require_lister()?;

        perm::list_users(self.inner.db())
    }

    /// The names of every group, sorted. Administrators and super-group members.
    pub fn list_groups(&self) -> AdbResult<Vec<String>> {
        self.require_lister()?;

        perm::list_groups(self.inner.db())
    }

    /// The groups `user` belongs to, sorted. Administrators and super-group members.
    pub fn user_groups(&self, user: &str) -> AdbResult<Vec<String>> {
        self.require_lister()?;

        perm::user_groups(self.inner.db(), user)
    }

    /// The members of `group`, sorted. Administrators and super-group members.
    pub fn group_members(&self, group: &str) -> AdbResult<Vec<String>> {
        self.require_lister()?;

        perm::group_members(self.inner.db(), group)
    }

    /// Removes a group: unassigns it from every user and strips it from every vnode
    /// ACL. The built-in master and super groups cannot be removed. Administrators only.
    pub fn remove_group(&self, name: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        perm::remove_group(self.inner.db(), key, name)
    }

    /// Removes a user together with **every value it owns**, so nothing is left
    /// orphaned. The built-in master and guest users cannot be removed. Administrators
    /// only (a master-group member may remove their own account).
    pub fn remove_user(&self, name: &str) -> AdbResult<()> {
        let key = self.admin_key()?;

        let uid = {
            let txn = self.inner.db().begin_read()?;
            let meta = txn.open_table(engine::META_TABLE)?;
            perm::uid_of(&meta, name)?
        }
        .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{name}'")))?;

        if uid == perm::MASTER_UID || uid == perm::GUEST_UID {
            return Err(AdbError::PermissionDenied(format!(
                "the built-in user '{name}' cannot be removed"
            )));
        }

        // The user tables to sweep for values this user owns.
        let tables: Vec<String> = {
            let txn = self.inner.db().begin_read()?;
            txn.list_tables()?
                .map(|handle| handle.name().to_string())
                .filter(|table| !is_reserved_table_name(table))
                .collect()
        };

        // One transaction: reap the owned values in every table, then drop the record.
        let txn = self.inner.db().begin_write()?;
        for table in &tables {
            crate::txn::reap_owned_in(&txn, table, uid, self.principal.as_ref())?;
        }
        {
            let mut meta = txn.open_table(engine::META_TABLE)?;
            perm::forget_user(&mut meta, key, uid)?;
        }

        let guard = self
            .inner
            .version_lock()
            .write()
            .map_err(|_| AdbError::CannotAccess(String::from("the version lock was poisoned")))?;

        txn.commit()?;
        self.inner.bump_generation();
        drop(guard);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{data::Scalar, AdbError, Value};

    #[test]
    fn open_table_rejects_empty_and_reserved_names() {
        let db = ArborDb::create_in_memory().unwrap();

        // The empty name and any `$`-prefixed name are rejected; a plain name is fine.
        assert!(matches!(db.open_table(""), Err(AdbError::InvalidTableName(_))));
        assert!(matches!(db.open_table("$metadata"), Err(AdbError::InvalidTableName(_))));
        assert!(matches!(db.open_table("$inodes"), Err(AdbError::InvalidTableName(_))));
        assert!(matches!(db.open_table("$anything"), Err(AdbError::InvalidTableName(_))));
        assert!(db.open_table("users").is_ok());
    }

    #[test]
    fn list_tables_reports_written_user_tables_only() {
        let db = ArborDb::create_in_memory().unwrap();

        // A fresh database exposes no user tables — the reserved `$metadata` table
        // is present but filtered out.
        assert!(db.list_tables().unwrap().is_empty());

        // A table opened but never written to is not materialized yet.
        let _pending = db.open_table("pending").unwrap();
        assert!(db.list_tables().unwrap().is_empty());

        // Two written tables show up in sorted order, without the reserved tables.
        for name in ["zeta", "alpha"] {
            let table = db.open_table(name).unwrap();
            let w = table.write().unwrap();
            w.store_value("k", &Value::Leaf(Scalar::I64(1))).unwrap();
            w.commit().unwrap();
        }

        assert_eq!(db.list_tables().unwrap(), vec!["alpha", "zeta"]);
    }
}
