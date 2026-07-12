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
    inode::{read_acl, read_meta, seal_mac, seal_sig, set_acl, Acl},
    perm::{self, Principal},
};

#[cfg(not(feature = "permissions"))]
use super::DirBuffer;

use redb::ReadableTable;
use smol_str::SmolStr;
use std::collections::BTreeMap;

#[cfg(feature = "permissions")]
use std::{cell::RefCell, collections::HashSet};

/// The per-transaction data table (borrows the write transaction).
pub(in super::super) type DataTable<'txn> = redb::Table<'txn, u128, EntryBytes>;

/// The tables a mutation writes through. Bundling the data table with the
/// (optional) per-a-node metadata table lets every a-node write keep that a-node's
/// `$inodes` entry — its timestamps — in step within the same transaction.
pub(in super::super) struct Context<'txn, 'a> {
    /// The data table this mutation writes a-node entries through.
    data: &'a mut DataTable<'txn>,

    /// The per-a-node metadata table, kept in step with `data`.
    #[cfg(feature = "entry-timestamps")]
    inodes: &'a mut InodeTable<'txn>,

    /// The table name, part of every `$inodes` key.
    #[cfg(feature = "entry-timestamps")]
    table: &'a str,

    /// One timestamp shared by every a-node this mutation touches.
    #[cfg(feature = "entry-timestamps")]
    now: i64,

    /// The transaction's write-back cache of dirty directories. Directory reads
    /// consult it first and directory writes buffer into it (flushed at commit),
    /// turning a bulk load under one parent from O(N²) into O(N). Absent under
    /// `permissions`, where directory reads must verify and ACL checks see each write.
    #[cfg(not(feature = "permissions"))]
    dirs: &'a DirBuffer,

    /// The identity performing the mutation; drives ACL enforcement.
    #[cfg(feature = "permissions")]
    principal: &'a Principal,

    /// a-nodes already integrity-verified in this mutation, so a directory touched
    /// twice (traversal then a child/kind probe) is MAC-verified once. Invalidated
    /// whenever an a-node is (re)written or removed. A `RefCell` because the read
    /// helpers take `&self`; the `Context` is a per-mutation stack local, never
    /// shared across threads.
    #[cfg(feature = "permissions")]
    verified: RefCell<HashSet<AKey>>,
}

impl<'txn, 'a> Context<'txn, 'a> {
    /// Bundles the data table with the per-a-node metadata table and one
    /// timestamp shared by every a-node this mutation touches.
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
            verified: RefCell::new(HashSet::new()),
        }
    }

    #[cfg(feature = "permissions")]
    pub(super) fn principal(&self) -> &'a Principal {
        self.principal
    }

    /// Forgets any memoized "already verified" mark for `akey` — called when its blob
    /// or ACL changes, so a later read in the same mutation re-verifies the new bytes.
    #[cfg(feature = "permissions")]
    fn invalidate_verified(&self, akey: AKey) {
        self.verified.borrow_mut().remove(&akey);
    }

    /// Bundles the data table with the per-a-node metadata table and the directory
    /// write-back cache (this build has timestamps but no permission system).
    #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
    pub(in super::super) fn new(
        data: &'a mut DataTable<'txn>,
        inodes: &'a mut InodeTable<'txn>,
        table: &'a str,
        dirs: &'a DirBuffer,
    ) -> Self {
        Self {
            data,
            inodes,
            table,
            now: timestamp_now(),
            dirs,
        }
    }

    /// Bundles the data table with the directory write-back cache (the metadata table
    /// exists only under the `entry-timestamps` feature).
    #[cfg(not(feature = "entry-timestamps"))]
    pub(in super::super) fn new(data: &'a mut DataTable<'txn>, dirs: &'a DirBuffer) -> Self {
        Self {
            data,
            dirs,
        }
    }

    /// Writes an a-node's entry blob and refreshes its inode — on creation all
    /// three times are set; on overwrite only `modified` moves.
    pub(super) fn put_entry(&mut self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        self.data.insert(u128::from(akey), entry)?;

        // The blob (and, below, its fresh seal) changed; a later read in this
        // mutation must re-verify rather than trust the pre-write memo.
        #[cfg(feature = "permissions")]
        self.invalidate_verified(akey);

        // Timestamps-only build: record the write time in the inode.
        #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
        inode::touch(self.inodes, self.table, akey, self.now)?;

        // Protected build: an authenticated user stamps the timestamps, the default
        // ACL, and both integrity tags in one read-modify-write of the inode; an
        // unrestricted handle (an unprotected database) only records the write time.
        #[cfg(feature = "permissions")]
        match self.principal {
            Principal::User(session) => {
                let table = self.table;
                let now = self.now;
                // The root carries no ACL; every other fresh a-node is owned by the writer.
                let owner = (akey != AKey::ROOT).then_some(session.uid());

                inode::stamp_and_seal(self.inodes, table, akey, now, owner, |acl| {
                    (
                        perm::integrity::mac_value(session.key(), table, akey, entry, acl),
                        perm::integrity::sign_value(session.signer(), table, akey, entry, acl),
                    )
                })?;
            }
            _ => inode::touch(self.inodes, self.table, akey, self.now)?,
        }

        Ok(())
    }

    /// Removes an a-node's entry blob together with its inode.
    pub(super) fn remove_entry(&mut self, akey: AKey) -> AdbResult<()> {
        self.data.remove(u128::from(akey))?;

        #[cfg(feature = "permissions")]
        self.invalidate_verified(akey);

        #[cfg(feature = "entry-timestamps")]
        inode::forget(self.inodes, self.table, akey)?;

        // Drop any buffered copy so the commit-time flush never resurrects it.
        #[cfg(not(feature = "permissions"))]
        self.dirs.forget(akey);

        Ok(())
    }

    /// Reads `akey`'s entry, first verifying its integrity tag (for a keyed
    /// principal), so a writer never trusts — nor launders into a fresh tag —
    /// a blob altered outside the library. `None` if the a-node is absent.
    pub(super) fn read_verified(&self, akey: AKey) -> AdbResult<Option<Vec<u8>>> {
        let entry = read_entry(&*self.data, akey)?;

        #[cfg(feature = "permissions")]
        if let Some(ref bytes) = entry {
            self.verify_integrity(akey, bytes)?;
        }

        Ok(entry)
    }

    /// Whether a-node `akey` exists — a bare presence probe that neither copies the
    /// entry nor verifies it (any actual read of its bytes still verifies). Lets a
    /// caller that only needs "is it there?" skip materializing a whole blob.
    pub(super) fn has_entry(&self, akey: AKey) -> AdbResult<bool> {
        // A directory mutated in this transaction lives in the write-back cache, which
        // is authoritative until the commit-time flush.
        #[cfg(not(feature = "permissions"))]
        if self.dirs.dirty() && self.dirs.read(|dirs| dirs.contains_key(&akey)) {
            return Ok(true);
        }

        Ok(self.data.get(u128::from(akey))?.is_some())
    }

    /// Fetches `akey`'s entry, verifies its integrity tag (for a keyed principal),
    /// and hands the **borrowed** entry bytes to `f`. Unlike [`read_verified`], it
    /// never copies the blob into an owned buffer — a read-only navigation (a child
    /// lookup, a kind probe, a directory listing) reads what it needs straight out
    /// of the engine page and drops the guard. `None` if the a-node is absent.
    ///
    /// [`read_verified`]: Self::read_verified
    fn with_entry<R>(&self, akey: AKey, f: impl FnOnce(&[u8]) -> AdbResult<R>) -> AdbResult<Option<R>> {
        let Some(guard) = self.data.get(u128::from(akey))? else {
            return Ok(None);
        };

        let entry = guard.value();

        #[cfg(feature = "permissions")]
        self.verify_integrity(akey, entry)?;

        f(entry).map(Some)
    }

    /// The child of directory `parent` named `name`, verifying `parent`'s
    /// integrity first. `None` if `parent` is absent, is a file, or has no such child.
    pub(super) fn child(&self, parent: AKey, name: &str) -> AdbResult<Option<AKey>> {
        // A directory buffered in this transaction is authoritative; read it there.
        #[cfg(not(feature = "permissions"))]
        if self.dirs.dirty()
            && let Some(found) = self
                .dirs
                .read(|dirs| dirs.get(&parent).map(|children| children.get(name).copied()))
        {
            return Ok(found);
        }

        Ok(self
            .with_entry(parent, |entry| {
                let (kind, payload) = entry_split(entry)?;
                if kind != EntryKind::Dir {
                    return Ok(None);
                }

                ArchivedDir::new(payload)?.get(name)
            })?
            .flatten())
    }

    /// The filesystem kind of `akey`, verifying its integrity first. `None` if
    /// the a-node is absent.
    pub(super) fn kind(&self, akey: AKey) -> AdbResult<Option<EntryKind>> {
        // A buffered a-node is always a directory (only directories buffer).
        #[cfg(not(feature = "permissions"))]
        if self.dirs.dirty() && self.dirs.read(|dirs| dirs.contains_key(&akey)) {
            return Ok(Some(EntryKind::Dir));
        }

        self.with_entry(akey, |entry| Ok(entry_split(entry)?.0))
    }

    /// Directory `akey`'s children as an owned map, verifying its integrity first
    /// (empty if the a-node is absent). Errors if `akey` is a file.
    pub(super) fn dir_children(&self, akey: AKey) -> AdbResult<BTreeMap<SmolStr, AKey>> {
        // A directory buffered in this transaction is authoritative.
        #[cfg(not(feature = "permissions"))]
        if self.dirs.dirty()
            && let Some(children) = self.dirs.read(|dirs| dirs.get(&akey).cloned())
        {
            return Ok(children);
        }

        let children = self.with_entry(akey, |entry| {
            let (kind, payload) = entry_split(entry)?;
            if kind != EntryKind::Dir {
                return Err(AdbError::CannotAccess(String::from(
                    "a path component is a file, not a directory",
                )));
            }

            Ok(ArchivedDir::new(payload)?
                .entries()?
                .into_iter()
                .map(|(name, child)| (SmolStr::from(name), child))
                .collect())
        })?;

        Ok(children.unwrap_or_default())
    }

    /// The child keys to recurse into when cascade-deleting `akey`, read borrowed
    /// (and verified) in one shot: `None` if the a-node is absent, an empty vec for a
    /// file, and its children for a directory. Deleting a file therefore never copies
    /// its (possibly large) blob just to learn it has no children.
    pub(super) fn cascade_children(&self, akey: AKey) -> AdbResult<Option<Vec<AKey>>> {
        // A buffered directory is authoritative: yield its cached children.
        #[cfg(not(feature = "permissions"))]
        if self.dirs.dirty()
            && let Some(children) = self
                .dirs
                .read(|dirs| dirs.get(&akey).map(|m| m.values().copied().collect::<Vec<_>>()))
        {
            return Ok(Some(children));
        }

        self.with_entry(akey, |entry| {
            let (kind, payload) = entry_split(entry)?;
            match kind {
                EntryKind::File => Ok(Vec::new()),
                EntryKind::Dir => Ok(ArchivedDir::new(payload)?
                    .entries()?
                    .into_iter()
                    .map(|(_, child)| child)
                    .collect()),
            }
        })
    }

    /// Whether directory writes should buffer in the write-back cache (an index-free
    /// database) rather than re-encode and write the blob on every link.
    #[cfg(not(feature = "permissions"))]
    pub(super) fn buffering(&self) -> bool {
        self.dirs.active()
    }

    /// Buffers `map` as directory `akey`'s whole child-map (a fresh or replaced
    /// directory), to be encoded and written once at commit.
    #[cfg(not(feature = "permissions"))]
    pub(super) fn buffer_dir(&self, akey: AKey, map: BTreeMap<SmolStr, AKey>) {
        self.dirs.write(|dirs| {
            dirs.insert(akey, map);
        });
    }

    /// Adds a `name → child` link to directory `parent` in the write-back cache,
    /// seeding it from the engine on first touch. A bulk of links then mutates one
    /// in-memory map (O(1) each) instead of re-encoding the directory blob per link.
    #[cfg(not(feature = "permissions"))]
    pub(super) fn dir_link(&self, parent: AKey, name: &str, child: AKey) -> AdbResult<()> {
        // Load the parent's current children on its first touch this transaction.
        let seed = if self.dirs.dirty() && self.dirs.read(|dirs| dirs.contains_key(&parent)) {
            None
        } else {
            Some(self.dir_children(parent)?)
        };

        self.dirs.write(|dirs| {
            let children = match seed {
                Some(loaded) => dirs.entry(parent).or_insert(loaded),
                None => dirs.get_mut(&parent).expect("the parent is buffered"),
            };

            children.insert(SmolStr::from(name), child);
        });

        Ok(())
    }

    /// Removes a `name` link from directory `parent` in the write-back cache, seeding
    /// it from the engine on first touch.
    #[cfg(not(feature = "permissions"))]
    pub(super) fn dir_unlink(&self, parent: AKey, name: &str) -> AdbResult<()> {
        let seed = if self.dirs.dirty() && self.dirs.read(|dirs| dirs.contains_key(&parent)) {
            None
        } else {
            Some(self.dir_children(parent)?)
        };

        self.dirs.write(|dirs| {
            let children = match seed {
                Some(loaded) => dirs.entry(parent).or_insert(loaded),
                None => dirs.get_mut(&parent).expect("the parent is buffered"),
            };

            children.remove(name);
        });

        Ok(())
    }

    /// Verifies `akey`'s integrity tag over `entry` and its current ACL, erroring
    /// with [`AdbError::Tampered`] on a mismatch. A no-op for an unrestricted
    /// handle or the keyless guest (a non-protected database has no integrity key).
    #[cfg(feature = "permissions")]
    fn verify_integrity(&self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        let Principal::User(session) = self.principal else {
            return Ok(());
        };

        // Skip a repeat verification of the same a-node within this mutation. The memo
        // is cleared whenever the a-node is written (see `invalidate_verified`), so it
        // never masks a change this transaction makes, and an external tamper is still
        // caught on the first read (a fresh transaction starts with an empty memo).
        if self.verified.borrow().contains(&akey) {
            return Ok(());
        }

        // The ACL and MAC come from one inode decode (the tag binds both).
        let (acl, mac, _sig) = read_meta(&*self.inodes, self.table, akey)?;
        let acl = acl.map(|acl| acl.encode()).unwrap_or_default();
        let expected = perm::integrity::mac_value(session.key(), self.table, akey, entry, &acl);

        match mac {
            Some(mac) if perm::integrity::ct_eq(&mac, &expected) => {
                self.verified.borrow_mut().insert(akey);

                Ok(())
            }
            _ => Err(AdbError::Tampered(format!(
                "integrity check failed for an a-node in table '{}'",
                self.table
            ))),
        }
    }

    /// Reads `akey`'s ACL — a missing one (e.g. an a-node predating protection)
    /// defaults to master-owned — and checks the principal holds the `needed` grade.
    #[cfg(feature = "permissions")]
    pub(super) fn check(&self, akey: AKey, needed: Rights) -> AdbResult<()> {
        let acl = read_acl(&*self.inodes, self.table, akey)?;

        perm::access::authorize(self.principal, akey, acl.as_ref(), needed)
    }

    /// Checks the principal may delete `akey` and, if it is a directory, every
    /// a-node beneath it — a cascade delete removes the whole subtree, so each
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

    /// Resolves `path` to an a-node key, checking `Access` on every directory
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

    /// Resolves `path` with no access checks (this build has no permission system),
    /// walking through [`child`](Self::child) so buffered directories are honoured.
    #[cfg(not(feature = "permissions"))]
    pub(super) fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
        let mut akey = AKey::ROOT;
        for name in path.names() {
            match self.child(akey, name.as_str())? {
                Some(child) => akey = child,
                None => return Ok(None),
            }
        }

        Ok(Some(akey))
    }

    /// Reads `akey`'s ACL, if it has one.
    #[cfg(feature = "permissions")]
    pub(super) fn acl(&self, akey: AKey) -> AdbResult<Option<Acl>> {
        read_acl(&*self.inodes, self.table, akey)
    }

    /// Replaces `akey`'s ACL.
    #[cfg(feature = "permissions")]
    pub(super) fn write_acl(&mut self, akey: AKey, acl: Acl) -> AdbResult<()> {
        // The ACL (which the integrity tag binds) changed; a later read re-verifies.
        self.invalidate_verified(akey);

        set_acl(self.inodes, self.table, akey, acl)
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

            let sig = perm::integrity::sign_value(session.signer(), self.table, akey, &entry, &acl);
            seal_sig(self.inodes, self.table, akey, sig)?;
        }

        Ok(())
    }

    /// Deletes every value owned by `uid` in this table, bypassing ACL checks
    /// (the caller must already be an authorized administrator).
    #[cfg(feature = "permissions")]
    pub(in super::super) fn reap_owned(&mut self, uid: u32) -> AdbResult<()> {
        let mut victims: Vec<(AKey, SmolStr)> = Vec::new();
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
