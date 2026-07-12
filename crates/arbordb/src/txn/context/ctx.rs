use crate::{
    codec::ArchivedDir,
    engine::{entry_split, read_entry, EntryBytes, EntryKind},
    error::{AdbError, AdbResult},
    path::APath,
    AKey,
};

#[cfg(feature = "entry-timestamps")]
use crate::{
    inode::{self, InodeTable},
    time::timestamp_now,
};

#[cfg(feature = "permissions")]
use super::table::{cascade_delete, collect_owned, unlink_child};

#[cfg(feature = "permissions")]
use crate::{
    acl::Rights,
    inode::{read_acl, read_mac, seal_mac, seal_sig, set_acl, set_default_acl, Acl},
    perm::{self, Principal},
};

use std::collections::BTreeMap;

/// The per-transaction data table (borrows the write transaction).
pub(in super::super) type DataTable<'txn> = redb::Table<'txn, u128, EntryBytes>;

/// The tables a mutation writes through. Bundling the data table with the
/// (optional) per-vnode metadata table lets every vnode write keep that vnode's
/// `$inodes` entry — its timestamps — in step within the same transaction.
pub(in super::super) struct Context<'txn, 'a> {
    /// The data table this mutation writes vnode entries through.
    data: &'a mut DataTable<'txn>,

    /// The per-vnode metadata table, kept in step with `data`.
    #[cfg(feature = "entry-timestamps")]
    inodes: &'a mut InodeTable<'txn>,

    /// The table name, part of every `$inodes` key.
    #[cfg(feature = "entry-timestamps")]
    table: &'a str,

    /// One timestamp shared by every vnode this mutation touches.
    #[cfg(feature = "entry-timestamps")]
    now: i64,

    /// The identity performing the mutation; drives ACL enforcement.
    #[cfg(feature = "permissions")]
    principal: &'a Principal,
}

impl<'txn, 'a> Context<'txn, 'a> {
    /// Bundles the data table with the per-vnode metadata table and one
    /// timestamp shared by every vnode this mutation touches.
    #[cfg(feature = "permissions")]
    pub(in super::super) fn new(
        data: &'a mut DataTable<'txn>,
        inodes: &'a mut InodeTable<'txn>,
        table: &'a str,
        principal: &'a Principal,
    ) -> Self {
        Self {
            data,
            inodes,
            table,
            now: timestamp_now(),
            principal,
        }
    }

    #[cfg(feature = "permissions")]
    pub(super) fn principal(&self) -> &'a Principal {
        self.principal
    }

    /// Bundles the data table with the per-vnode metadata table (this build has
    /// timestamps but no permission system, so there is no principal).
    #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
    pub(in super::super) fn new(
        data: &'a mut DataTable<'txn>,
        inodes: &'a mut InodeTable<'txn>,
        table: &'a str,
    ) -> Self {
        Self {
            data,
            inodes,
            table,
            now: timestamp_now(),
        }
    }

    /// Bundles the data table alone (the metadata table exists only under the
    /// `entry-timestamps` feature).
    #[cfg(not(feature = "entry-timestamps"))]
    pub(in super::super) fn new(data: &'a mut DataTable<'txn>) -> Self {
        Self {
            data,
        }
    }

    /// Writes a vnode's entry blob and refreshes its inode — on creation all
    /// three times are set; on overwrite only `modified` moves.
    pub(super) fn put_entry(&mut self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        self.data.insert(u128::from(akey), entry)?;

        #[cfg(feature = "entry-timestamps")]
        inode::touch(self.inodes, self.table, akey, self.now)?;

        #[cfg(feature = "permissions")]
        {
            self.stamp_default_acl(akey)?;
            self.seal_integrity(akey, entry)?;
        }

        Ok(())
    }

    /// Removes a vnode's entry blob together with its inode.
    pub(super) fn remove_entry(&mut self, akey: AKey) -> AdbResult<()> {
        self.data.remove(u128::from(akey))?;

        #[cfg(feature = "entry-timestamps")]
        inode::forget(self.inodes, self.table, akey)?;

        Ok(())
    }

    /// Reads `akey`'s entry, first verifying its integrity tag (for a keyed
    /// principal), so a writer never trusts — nor launders into a fresh tag —
    /// a blob altered outside the library. `None` if the vnode is absent.
    pub(super) fn read_verified(&self, akey: AKey) -> AdbResult<Option<Vec<u8>>> {
        let entry = read_entry(&*self.data, akey)?;

        #[cfg(feature = "permissions")]
        if let Some(ref bytes) = entry {
            self.verify_integrity(akey, bytes)?;
        }

        Ok(entry)
    }

    /// The child of directory `parent` named `name`, verifying `parent`'s
    /// integrity first. `None` if `parent` is absent, is a file, or has no such child.
    pub(super) fn child(&self, parent: AKey, name: &str) -> AdbResult<Option<AKey>> {
        let Some(entry) = self.read_verified(parent)? else {
            return Ok(None);
        };

        let (kind, payload) = entry_split(&entry)?;
        if kind != EntryKind::Dir {
            return Ok(None);
        }

        ArchivedDir::new(payload)?.get(name)
    }

    /// The filesystem kind of `akey`, verifying its integrity first. `None` if
    /// the vnode is absent.
    pub(super) fn kind(&self, akey: AKey) -> AdbResult<Option<EntryKind>> {
        match self.read_verified(akey)? {
            Some(entry) => Ok(Some(entry_split(&entry)?.0)),
            None => Ok(None),
        }
    }

    /// Directory `akey`'s children as an owned map, verifying its integrity first
    /// (empty if the vnode is absent). Errors if `akey` is a file.
    pub(super) fn dir_children(&self, akey: AKey) -> AdbResult<BTreeMap<String, AKey>> {
        let Some(entry) = self.read_verified(akey)? else {
            return Ok(BTreeMap::new());
        };

        let (kind, payload) = entry_split(&entry)?;
        if kind != EntryKind::Dir {
            return Err(AdbError::CannotAccess(String::from(
                "a path component is a file, not a directory",
            )));
        }

        Ok(ArchivedDir::new(payload)?
            .entries()?
            .into_iter()
            .map(|(name, child)| (name.to_string(), child))
            .collect())
    }

    /// Verifies `akey`'s integrity tag over `entry` and its current ACL, erroring
    /// with [`AdbError::Tampered`] on a mismatch. A no-op for an unrestricted
    /// handle or the keyless guest (a non-protected database has no integrity key).
    #[cfg(feature = "permissions")]
    fn verify_integrity(&self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        let Principal::User(session) = self.principal else {
            return Ok(());
        };

        let acl = read_acl(&*self.inodes, self.table, akey)?
            .map(|acl| acl.encode())
            .unwrap_or_default();
        let stored = read_mac(&*self.inodes, self.table, akey)?;
        let expected = perm::integrity::mac_value(session.key(), self.table, akey, entry, &acl);

        match stored {
            Some(mac) if perm::integrity::ct_eq(&mac, &expected) => Ok(()),
            _ => Err(AdbError::Tampered(format!(
                "integrity check failed for a vnode in table '{}'",
                self.table
            ))),
        }
    }

    /// Reads `akey`'s ACL — a missing one (e.g. a vnode predating protection)
    /// defaults to master-owned — and checks the principal holds the `needed` grade.
    #[cfg(feature = "permissions")]
    pub(super) fn check(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        let acl = read_acl(&*self.inodes, self.table, akey)?;

        perm::access::authorize(self.principal, akey, acl.as_ref(), needed)
    }

    /// Checks the principal may delete `akey` and, if it is a directory, every
    /// vnode beneath it — a cascade delete removes the whole subtree, so each
    /// node in it must be deletable.
    #[cfg(feature = "permissions")]
    pub(super) fn check_deletable(&self, akey: AKey) -> AdbResult<()> {
        self.check(akey, Rights::Delete)?;

        if self.kind(akey)? == Some(EntryKind::Dir) {
            for (_, child) in self.dir_children(akey)? {
                self.check_deletable(child)?;
            }
        }

        Ok(())
    }

    /// Resolves `path` to a vnode key, checking `Access` on every directory
    /// traversed. `None` if a component along the way is missing.
    #[cfg(feature = "permissions")]
    pub(super) fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
        let mut akey = AKey::ROOT;
        for name in path.names() {
            self.check(akey, Rights::Access)?;
            match self.child(akey, name.as_str())? {
                Some(child) => akey = child,
                None => return Ok(None),
            }
        }

        Ok(Some(akey))
    }

    /// Resolves `path` with no access checks (this build has no permission system).
    #[cfg(not(feature = "permissions"))]
    pub(super) fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
        crate::engine::resolve(&*self.data, path)
    }

    /// Stamps the default ACL (owner = the authenticated user, its first group)
    /// on a freshly created vnode; a no-op for an unrestricted handle or when an
    /// ACL is already present, so an overwrite preserves it.
    #[cfg(feature = "permissions")]
    fn stamp_default_acl(&mut self, akey: AKey) -> AdbResult<()> {
        // The root is special and carries no ACL.
        if akey == AKey::ROOT {
            return Ok(());
        }

        if let Principal::User(session) = self.principal {
            set_default_acl(self.inodes, self.table, akey, session.uid())?;
        }

        Ok(())
    }

    /// Reads `akey`'s ACL, if it has one.
    #[cfg(feature = "permissions")]
    pub(super) fn acl(&self, akey: AKey) -> AdbResult<Option<Acl>> {
        read_acl(&*self.inodes, self.table, akey)
    }

    /// Replaces `akey`'s ACL.
    #[cfg(feature = "permissions")]
    pub(super) fn write_acl(&mut self, akey: AKey, acl: Acl) -> AdbResult<()> {
        set_acl(self.inodes, self.table, akey, acl)
    }

    /// Seals `akey`'s integrity tags over its entry bytes and current ACL: the
    /// keyed MAC an authenticated reader verifies *and* the signature a guest
    /// verifies. A no-op for an unrestricted handle — a non-protected database has
    /// no keys, so nothing is sealed. Only an authenticated user reaches here (a
    /// guest cannot write).
    #[cfg(feature = "permissions")]
    fn seal_integrity(&mut self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        if let Principal::User(session) = self.principal {
            let acl = read_acl(&*self.inodes, self.table, akey)?
                .map(|acl| acl.encode())
                .unwrap_or_default();

            let mac = perm::integrity::mac_value(session.key(), self.table, akey, entry, &acl);
            seal_mac(self.inodes, self.table, akey, mac)?;

            let sig = perm::integrity::sign_value(session.sign_seed(), self.table, akey, entry, &acl);
            seal_sig(self.inodes, self.table, akey, sig)?;
        }

        Ok(())
    }

    /// Re-seals `akey`'s integrity tags after an ACL change: the entry bytes are
    /// unchanged, but both tags also bind the ACL, so each must be recomputed.
    #[cfg(feature = "permissions")]
    pub(super) fn reseal_integrity(&mut self, akey: AKey) -> AdbResult<()> {
        if let Principal::User(session) = self.principal {
            let Some(entry) = read_entry(&*self.data, akey)? else {
                return Ok(());
            };

            let acl = read_acl(&*self.inodes, self.table, akey)?
                .map(|acl| acl.encode())
                .unwrap_or_default();

            let mac = perm::integrity::mac_value(session.key(), self.table, akey, &entry, &acl);
            seal_mac(self.inodes, self.table, akey, mac)?;

            let sig = perm::integrity::sign_value(session.sign_seed(), self.table, akey, &entry, &acl);
            seal_sig(self.inodes, self.table, akey, sig)?;
        }

        Ok(())
    }

    /// Deletes every value owned by `uid` in this table, bypassing ACL checks
    /// (the caller must already be an authorized administrator).
    #[cfg(feature = "permissions")]
    pub(in super::super) fn reap_owned(&mut self, uid: u32) -> AdbResult<()> {
        let mut victims: Vec<(AKey, String)> = Vec::new();
        collect_owned(self, AKey::ROOT, uid, &mut victims)?;

        for (parent, name) in victims {
            if let Some(child) = self.child(parent, &name)? {
                cascade_delete(self, child)?;
                unlink_child(self, parent, &name)?;
            }
        }

        Ok(())
    }
}
