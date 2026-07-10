//! The opaque write transaction: serialized mutation of one table.

use super::rooted::RootedWrite;
use crate::{
    access::{MemWriter, MutCursor, Writer},
    codec::{decode, encode, encode_dir, ArchivedDir, ArchivedValue},
    data::{AData, AMut, Scalar},
    db::DbInner,
    engine::{
        data_def,
        dir_entry,
        fetch_entry_kind,
        file_entry,
        read_entry,
        resolve,
        entry_split,
        EntryBytes,
        EntryKind,
        INDEX_TABLE,
        META_TABLE,
    },
    error::{AdbError, AdbResult},
    index::{
        maintenance,
        registry::{self, IndexEntry},
    },
    path::{APath, IntoArborPath, VPath},
    value::Value,
    AKey,
};

#[cfg(feature = "entry-timestamps")]
use crate::{
    inode::{self, InodeTable, INODES_TABLE},
    time::timestamp_now,
};

#[cfg(feature = "permissions")]
use crate::{
    inode::{mode_to_bits, read_acl, read_mac, seal_mac, seal_sig, set_acl, set_default_acl, Acl, Right},
    perm::{self, Principal},
};

use redb::WriteTransaction;
use std::{collections::BTreeMap, sync::Arc};

/// The per-transaction data table (borrows the write transaction).
type DataTable<'txn> = redb::Table<'txn, u128, EntryBytes>;

/// The virtual-filesystem write helpers backing the public [`WriteTxn`] ops.
///
/// These routines operate directly on the raw [`DataTable`] — resolving,
/// linking, rewriting, and cascading over directory and file nodes — rather
/// than on `&self`. Grouping them here keeps them off [`WriteTxn`]'s surface
/// while letting each public op compose them.
mod table {
    use super::*;

    /// The tables a mutation writes through. Bundling the data table with the
    /// (optional) per-vnode metadata table lets every vnode write keep that vnode's
    /// `$inodes` entry — its timestamps — in step within the same transaction.
    pub(super) struct Ctx<'txn, 'a> {
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

    impl<'txn, 'a> Ctx<'txn, 'a> {
        /// Bundles the data table with the per-vnode metadata table and one
        /// timestamp shared by every vnode this mutation touches.
        #[cfg(feature = "permissions")]
        pub(super) fn new(
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

        /// Bundles the data table with the per-vnode metadata table (this build has
        /// timestamps but no permission system, so there is no principal).
        #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
        pub(super) fn new(data: &'a mut DataTable<'txn>, inodes: &'a mut InodeTable<'txn>, table: &'a str) -> Self {
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
        pub(super) fn new(data: &'a mut DataTable<'txn>) -> Self {
            Self {
                data,
            }
        }

        /// Writes a vnode's entry blob and refreshes its inode — on creation all
        /// three times are set; on overwrite only `modified` moves.
        fn put_entry(&mut self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
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
        fn remove_entry(&mut self, akey: AKey) -> AdbResult<()> {
            self.data.remove(u128::from(akey))?;

            #[cfg(feature = "entry-timestamps")]
            inode::forget(self.inodes, self.table, akey)?;

            Ok(())
        }

        /// Reads `akey`'s entry, first verifying its integrity tag (for a keyed
        /// principal), so a writer never trusts — nor launders into a fresh tag —
        /// a blob altered outside the library. `None` if the vnode is absent.
        fn read_verified(&self, akey: AKey) -> AdbResult<Option<Vec<u8>>> {
            let entry = read_entry(&*self.data, akey)?;

            #[cfg(feature = "permissions")]
            if let Some(ref bytes) = entry {
                self.verify_integrity(akey, bytes)?;
            }

            Ok(entry)
        }

        /// The child of directory `parent` named `name`, verifying `parent`'s
        /// integrity first. `None` if `parent` is absent, is a file, or has no such child.
        fn child(&self, parent: AKey, name: &str) -> AdbResult<Option<AKey>> {
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
        fn kind(&self, akey: AKey) -> AdbResult<Option<EntryKind>> {
            match self.read_verified(akey)? {
                Some(entry) => Ok(Some(entry_split(&entry)?.0)),
                None => Ok(None),
            }
        }

        /// Directory `akey`'s children as an owned map, verifying its integrity first
        /// (empty if the vnode is absent). Errors if `akey` is a file.
        fn dir_children(&self, akey: AKey) -> AdbResult<BTreeMap<String, AKey>> {
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
        /// defaults to master-owned — and checks the principal holds `right`.
        #[cfg(feature = "permissions")]
        fn check(&self, akey: AKey, right: Right) -> AdbResult<()> {
            let acl = read_acl(&*self.inodes, self.table, akey)?;

            perm::access::authorize(self.principal, akey, acl.as_ref(), right)
        }

        /// Resolves `path` to a vnode key, checking `walk` on every directory
        /// traversed. `None` if a component along the way is missing.
        #[cfg(feature = "permissions")]
        fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
            let mut akey = AKey::ROOT;
            for name in path.names() {
                self.check(akey, Right::Walk)?;
                match self.child(akey, name.as_str())? {
                    Some(child) => akey = child,
                    None => return Ok(None),
                }
            }

            Ok(Some(akey))
        }

        /// Resolves `path` with no access checks (this build has no permission system).
        #[cfg(not(feature = "permissions"))]
        fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>> {
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
        fn acl(&self, akey: AKey) -> AdbResult<Option<Acl>> {
            read_acl(&*self.inodes, self.table, akey)
        }

        /// Replaces `akey`'s ACL.
        #[cfg(feature = "permissions")]
        fn write_acl(&mut self, akey: AKey, acl: Acl) -> AdbResult<()> {
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
        fn reseal_integrity(&mut self, akey: AKey) -> AdbResult<()> {
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
        pub(super) fn reap_owned(&mut self, uid: u32) -> AdbResult<()> {
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

    /// Stores `value` as a file at `path` (a non-root path), creating parents and
    /// replacing whatever was there.
    pub(super) fn store_value_into(ctx: &mut Ctx, path: &APath, value: &Value) -> AdbResult<()> {
        let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");
        let parent = ensure_dir(ctx, &parent_path)?;

        #[cfg(feature = "permissions")]
        ctx.check(parent, Right::Walk)?;

        let akey = match ctx.child(parent, name)? {
            Some(node) => {
                if ctx.kind(node)? == Some(EntryKind::File) {
                    #[cfg(feature = "permissions")]
                    ctx.check(node, Right::Write)?;

                    node
                } else {
                    #[cfg(feature = "permissions")]
                    ctx.check(parent, Right::Write)?;

                    cascade_delete(ctx, node)?;
                    let fresh = AKey::generate();
                    link_child(ctx, parent, name, fresh)?;
                    fresh
                }
            }
            None => {
                #[cfg(feature = "permissions")]
                ctx.check(parent, Right::Write)?;

                let fresh = AKey::generate();
                link_child(ctx, parent, name, fresh)?;
                fresh
            }
        };

        let entry = file_entry(&encode(value));
        ctx.put_entry(akey, entry.as_slice())?;

        Ok(())
    }

    /// Sets the scalar at `at` inside the file at `path`. When the new scalar keeps the
    /// current leaf's byte width the blob is patched in place — no decode, no
    /// re-encode; otherwise the value is decoded, updated, and re-encoded. Creates the
    /// file (and its parents) when it does not exist yet.
    pub(super) fn put_scalar_into(ctx: &mut Ctx, path: &APath, at: &VPath, scalar: &Scalar) -> AdbResult<()> {
        let akey = ctx.resolve(path)?;
        let entry = match akey {
            Some(akey) => ctx.read_verified(akey)?,
            None => None,
        };

        let mut value = match (akey, entry) {
            (Some(akey), Some(mut entry)) => {
                #[cfg(feature = "permissions")]
                ctx.check(akey, Right::Write)?;

                // Fast path: a leaf that keeps its width is patched in place — every
                // other offset in the blob stays valid, so nothing is re-encoded.
                if patch_scalar(&mut entry, at, scalar)? {
                    ctx.put_entry(akey, entry.as_slice())?;

                    return Ok(());
                }

                // Slow path: decode the (untouched) blob to re-encode it below.
                let (kind, payload) = entry_split(&entry)?;
                match kind {
                    EntryKind::File => decode(payload)?,
                    EntryKind::Dir => {
                        return Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file")));
                    }
                }
            }
            _ => Value::default(),
        };

        value.set_value(at, Value::Leaf(scalar.clone()));

        store_value_into(ctx, path, &value)
    }

    /// Patches `entry` in place when its file's leaf at `at` keeps its encoded width
    /// under `scalar`, overwriting just that leaf's bytes and returning `true`. Returns
    /// `false`, leaving `entry` untouched, when the fast path does not apply (a
    /// directory, an absent or non-leaf path, or a width change) — the caller then
    /// re-encodes.
    pub(super) fn patch_scalar(entry: &mut [u8], at: &VPath, scalar: &Scalar) -> AdbResult<bool> {
        let mut encoded = Vec::new();
        scalar.encode(&mut encoded);

        // Locate the leaf's bytes, then drop the borrow before overwriting them.
        let start = {
            let (kind, payload) = entry_split(entry)?;
            if kind != EntryKind::File {
                return Ok(false);
            }

            let Some((scalar_off, current_len)) = ArchivedValue::new(payload)?.leaf_scalar_span(at)? else {
                return Ok(false);
            };

            if encoded.len() != current_len {
                return Ok(false);
            }

            // `scalar_off` is relative to the payload; the entry prefixes it with a
            // one-byte kind tag, so shift past that header.
            (entry.len() - payload.len()) + scalar_off
        };

        entry[start..start + encoded.len()].copy_from_slice(&encoded);

        Ok(true)
    }

    /// Removes the vnode at `path` (a non-root path) and its subtree. Returns whether
    /// anything was removed.
    pub(super) fn rm_into(ctx: &mut Ctx, path: &APath) -> AdbResult<bool> {
        let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");

        let Some(parent) = ctx.resolve(&parent_path)? else {
            return Ok(false);
        };

        #[cfg(feature = "permissions")]
        ctx.check(parent, Right::Walk)?;

        let Some(akey) = ctx.child(parent, name)? else {
            return Ok(false);
        };

        #[cfg(feature = "permissions")]
        ctx.check(parent, Right::Write)?;

        cascade_delete(ctx, akey)?;
        unlink_child(ctx, parent, name)?;

        Ok(true)
    }

    /// Relinks the vnode at `src` to `dst`, keeping its identity.
    pub(super) fn mv_into(ctx: &mut Ctx, src: &APath, dst: &APath) -> AdbResult<()> {
        let (src_parent_path, src_name) = src.split_last().expect("a non-root path has a parent and a name");
        let (dst_parent_path, dst_name) = dst.split_last().expect("a non-root path has a parent and a name");

        let Some(src_parent) = ctx.resolve(&src_parent_path)? else {
            return Err(AdbError::ValueNotFound(src.clone()));
        };

        #[cfg(feature = "permissions")]
        ctx.check(src_parent, Right::Walk)?;

        let Some(akey) = ctx.child(src_parent, src_name)? else {
            return Err(AdbError::ValueNotFound(src.clone()));
        };

        let dst_parent = ensure_dir(ctx, &dst_parent_path)?;

        #[cfg(feature = "permissions")]
        ctx.check(dst_parent, Right::Write)?;

        if let Some(existing) = ctx.child(dst_parent, dst_name)? {
            cascade_delete(ctx, existing)?;
        }

        link_child(ctx, dst_parent, dst_name, akey)?;

        #[cfg(feature = "permissions")]
        ctx.check(src_parent, Right::Write)?;

        unlink_child(ctx, src_parent, src_name)?;

        Ok(())
    }

    /// Deep-copies the subtree at `src` to `dst` under fresh identities.
    pub(super) fn cp_into(ctx: &mut Ctx, src: &APath, dst: &APath) -> AdbResult<()> {
        let (dst_parent_path, dst_name) = dst.split_last().expect("a non-root path has a parent and a name");

        let Some(src_akey) = ctx.resolve(src)? else {
            return Err(AdbError::ValueNotFound(src.clone()));
        };

        let dst_parent = ensure_dir(ctx, &dst_parent_path)?;

        #[cfg(feature = "permissions")]
        ctx.check(dst_parent, Right::Write)?;

        if let Some(existing) = ctx.child(dst_parent, dst_name)? {
            cascade_delete(ctx, existing)?;
        }

        let copy = deep_copy(ctx, src_akey)?;
        link_child(ctx, dst_parent, dst_name, copy)?;

        Ok(())
    }

    /// Writes `map` as directory `akey`'s children.
    pub(super) fn put_dir(ctx: &mut Ctx, akey: AKey, map: &BTreeMap<String, AKey>) -> AdbResult<()> {
        let entry = dir_entry(&encode_dir(map));
        ctx.put_entry(akey, entry.as_slice())?;

        Ok(())
    }

    /// Adds (or replaces) a `name → child` link in directory `parent`.
    pub(super) fn link_child(ctx: &mut Ctx, parent: AKey, name: &str, child: AKey) -> AdbResult<()> {
        let mut map = ctx.dir_children(parent)?;
        map.insert(name.to_string(), child);

        put_dir(ctx, parent, &map)
    }

    /// Removes the `name` link from directory `parent`.
    pub(super) fn unlink_child(ctx: &mut Ctx, parent: AKey, name: &str) -> AdbResult<()> {
        let mut map = ctx.dir_children(parent)?;
        map.remove(name);

        put_dir(ctx, parent, &map)
    }

    /// Ensures the root directory exists.
    pub(super) fn ensure_root(ctx: &mut Ctx) -> AdbResult<()> {
        if ctx.read_verified(AKey::ROOT)?.is_none() {
            put_dir(ctx, AKey::ROOT, &BTreeMap::new())?;
        }

        Ok(())
    }

    /// Ensures every directory along `path` exists, returning the deepest one's key.
    pub(super) fn ensure_dir(ctx: &mut Ctx, path: &APath) -> AdbResult<AKey> {
        ensure_root(ctx)?;

        let mut akey = AKey::ROOT;
        for name in path.names() {
            #[cfg(feature = "permissions")]
            ctx.check(akey, Right::Walk)?;

            match ctx.child(akey, name.as_str())? {
                Some(child) => {
                    if ctx.kind(child)? != Some(EntryKind::Dir) {
                        return Err(AdbError::CannotAccess(format!("'{name}' is a file, not a directory")));
                    }

                    akey = child;
                }
                None => {
                    #[cfg(feature = "permissions")]
                    ctx.check(akey, Right::Write)?;

                    let child = AKey::generate();
                    put_dir(ctx, child, &BTreeMap::new())?;
                    link_child(ctx, akey, name.as_str(), child)?;
                    akey = child;
                }
            }
        }

        Ok(akey)
    }

    /// Removes vnode `akey` and, if it is a directory, its whole subtree.
    pub(super) fn cascade_delete(ctx: &mut Ctx, akey: AKey) -> AdbResult<()> {
        let children = match ctx.read_verified(akey)? {
            Some(entry) => {
                let (kind, payload) = entry_split(&entry)?;
                match kind {
                    EntryKind::Dir => ArchivedDir::new(payload)?
                        .entries()?
                        .into_iter()
                        .map(|(_, child)| child)
                        .collect::<Vec<_>>(),
                    EntryKind::File => Vec::new(),
                }
            }
            None => return Ok(()),
        };

        for child in children {
            cascade_delete(ctx, child)?;
        }

        ctx.remove_entry(akey)?;

        Ok(())
    }

    /// Deep-copies vnode `akey` (a file's blob verbatim, a directory recursively) under
    /// a freshly generated key, returning that key.
    pub(super) fn deep_copy(ctx: &mut Ctx, akey: AKey) -> AdbResult<AKey> {
        #[cfg(feature = "permissions")]
        ctx.check(akey, Right::Read)?;

        let entry = ctx
            .read_verified(akey)?
            .ok_or_else(|| AdbError::Corrupt("copying a missing vnode".into()))?;
        let (kind, payload) = entry_split(&entry)?;
        let fresh = AKey::generate();

        match kind {
            EntryKind::File => {
                ctx.put_entry(fresh, entry.as_slice())?;
            }
            EntryKind::Dir => {
                let children: Vec<(String, AKey)> = ArchivedDir::new(payload)?
                    .entries()?
                    .into_iter()
                    .map(|(name, child)| (name.to_string(), child))
                    .collect();

                let mut copied = BTreeMap::new();
                for (name, child) in children {
                    copied.insert(name, deep_copy(ctx, child)?);
                }

                put_dir(ctx, fresh, &copied)?;
            }
        }

        Ok(fresh)
    }

    /// Changes the owner of the vnode at `path` to `new_uid`.
    #[cfg(feature = "permissions")]
    pub(super) fn chown_into(ctx: &mut Ctx, path: &APath, new_uid: u32) -> AdbResult<()> {
        let akey = ctx
            .resolve(path)?
            .ok_or_else(|| AdbError::ValueNotFound(path.clone()))?;
        if akey == AKey::ROOT {
            return Err(AdbError::PermissionDenied(String::from(
                "the root ACL cannot be changed",
            )));
        }

        // Verify the target before its ACL changes: the value MAC binds the ACL, so
        // re-sealing afterwards must not launder a blob altered outside the library.
        ctx.read_verified(akey)?;

        let mut acl = ctx
            .acl(akey)?
            .ok_or_else(|| AdbError::CannotAccess(String::from("this vnode has no ACL")))?;

        perm::access::authorize_chown(ctx.principal, &acl)?;
        acl.set_owner(new_uid);

        ctx.write_acl(akey, acl)?;
        ctx.reseal_integrity(akey)
    }

    /// Changes the group of the vnode at `path` to `new_gid` (`None` clears it).
    #[cfg(feature = "permissions")]
    pub(super) fn chgrp_into(ctx: &mut Ctx, path: &APath, new_gid: Option<u32>) -> AdbResult<()> {
        let akey = ctx
            .resolve(path)?
            .ok_or_else(|| AdbError::ValueNotFound(path.clone()))?;
        if akey == AKey::ROOT {
            return Err(AdbError::PermissionDenied(String::from(
                "the root ACL cannot be changed",
            )));
        }

        // Verify the target before its ACL changes: the value MAC binds the ACL, so
        // re-sealing afterwards must not launder a blob altered outside the library.
        ctx.read_verified(akey)?;

        let mut acl = ctx
            .acl(akey)?
            .ok_or_else(|| AdbError::CannotAccess(String::from("this vnode has no ACL")))?;

        perm::access::authorize_chgrp(ctx.principal, &acl, new_gid)?;
        acl.set_group(new_gid);

        ctx.write_acl(akey, acl)?;
        ctx.reseal_integrity(akey)
    }

    /// Sets the mode bits of the vnode at `path`. Requires `write` on the vnode.
    #[cfg(feature = "permissions")]
    pub(super) fn chmod_into(ctx: &mut Ctx, path: &APath, mode: u16) -> AdbResult<()> {
        let akey = ctx
            .resolve(path)?
            .ok_or_else(|| AdbError::ValueNotFound(path.clone()))?;
        if akey == AKey::ROOT {
            return Err(AdbError::PermissionDenied(String::from(
                "the root ACL cannot be changed",
            )));
        }

        // Verify the target before its ACL changes: the value MAC binds the ACL, so
        // re-sealing afterwards must not launder a blob altered outside the library.
        ctx.read_verified(akey)?;

        let mut acl = ctx
            .acl(akey)?
            .ok_or_else(|| AdbError::CannotAccess(String::from("this vnode has no ACL")))?;

        perm::access::authorize(ctx.principal, akey, Some(&acl), Right::Write)?;
        acl.set_mode(mode);

        ctx.write_acl(akey, acl)?;
        ctx.reseal_integrity(akey)
    }

    /// Recursively collects the `(parent, name)` of every top-most vnode owned by
    /// `uid` under directory `dir`. An owned directory is collected whole (and not
    /// descended into); a non-owned directory is descended to find owned vnodes.
    #[cfg(feature = "permissions")]
    pub(super) fn collect_owned(ctx: &Ctx, dir: AKey, uid: u32, out: &mut Vec<(AKey, String)>) -> AdbResult<()> {
        for (name, child) in ctx.dir_children(dir)? {
            if ctx.acl(child)?.map(|acl| acl.owner()) == Some(uid) {
                out.push((dir, name));
            } else if ctx.kind(child)? == Some(EntryKind::Dir) {
                collect_owned(ctx, child, uid, out)?;
            }
        }

        Ok(())
    }
}

/// Deletes every value owned by `uid` in `table`, within `txn`, bypassing ACL
/// checks (the caller must already be an authorized administrator). Used by user
/// removal so that no value is left orphaned.
#[cfg(feature = "permissions")]
pub(crate) fn reap_owned_in(txn: &WriteTransaction, table: &str, uid: u32, principal: &Principal) -> AdbResult<()> {
    let mut data = txn.open_table(data_def(table))?;
    let mut inodes = txn.open_table(INODES_TABLE)?;
    let mut ctx = table::Ctx::new(&mut data, &mut inodes, table, principal);

    ctx.reap_owned(uid)
}

/// A write transaction over one table. Changes become durable on [`commit`](WriteTxn::commit).
pub struct WriteTxn {
    /// The underlying engine write transaction.
    txn:   WriteTransaction,
    /// The table this transaction writes.
    table: String,
    /// The database-wide shared state (for the generation bump on commit).
    inner: Arc<DbInner>,

    /// The identity performing the writes; drives ACL enforcement.
    #[cfg(feature = "permissions")]
    principal: Arc<Principal>,
}

impl WriteTxn {
    pub(crate) fn new(
        txn: WriteTransaction,
        table: String,
        inner: Arc<DbInner>,
        #[cfg(feature = "permissions")] principal: Arc<Principal>,
    ) -> Self {
        Self {
            txn,
            table,
            inner,
            #[cfg(feature = "permissions")]
            principal,
        }
    }

    /// Stores a typed value as a file at `path`, creating parent directories as
    /// needed and replacing whatever was there. The value is decomposed into an
    /// in-memory tree, then encoded to one blob.
    pub fn store<T: AData>(&self, path: impl IntoArborPath, value: &T) -> AdbResult<()> {
        let writer = MemWriter::new();
        value.store(&writer, &VPath::root())?;

        self.store_value(path, &writer.into_value())
    }

    /// Stores a dynamic [`Value`] as a file at `path`, creating parent directories
    /// as needed. Replaces whatever was there: a file overwrite keeps the vnode's
    /// identity; a directory is removed with its whole subtree first.
    pub fn store_value(&self, path: impl IntoArborPath, value: &Value) -> AdbResult<()> {
        self.store_value_at(&path.into_arbor_path()?, value)
    }

    /// Stores `value` as a file at an already-parsed access path.
    pub(crate) fn store_value_at(&self, path: &APath, value: &Value) -> AdbResult<()> {
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot store a file at the root")));
        }

        self.reindex_around(std::slice::from_ref(path), |ctx| {
            table::store_value_into(ctx, path, value)
        })
    }

    /// Sets the scalar at `at` inside the file at `path`. When the new scalar encodes
    /// to the same width as the one already there, the blob is patched in place — no
    /// decode, no re-encode — otherwise the value is decoded, updated, and re-encoded.
    /// Registered indexes are maintained across either path.
    pub(crate) fn put_scalar_at(&self, path: &APath, at: &VPath, scalar: Scalar) -> AdbResult<()> {
        self.reindex_around(std::slice::from_ref(path), |ctx| {
            table::put_scalar_into(ctx, path, at, &scalar)
        })
    }

    /// Opens a mutable accessor over the file at `path`, or `None` if absent. A scalar
    /// overwrite through the accessor that keeps the leaf's byte width patches the blob
    /// in place (no decode, no re-encode); a structural change still rewrites it. The
    /// accessor borrows the transaction, so drop it before `commit`.
    pub fn fetch_mut<'t, A: AMut<'t>>(&'t self, path: impl IntoArborPath) -> AdbResult<Option<A>> {
        let apath = path.into_arbor_path()?;

        // Presence check without decoding the value — resolve the vnode and read its
        // kind alone.
        {
            let table = self.txn.open_table(data_def(&self.table))?;
            match resolve(&table, &apath)? {
                Some(akey) => match fetch_entry_kind(&table, akey)? {
                    Some(EntryKind::File) => {}
                    Some(EntryKind::Dir) => {
                        return Err(AdbError::CannotAccess(format!("'{apath}' is a directory, not a file")));
                    }
                    None => return Ok(None),
                },
                None => return Ok(None),
            }
        }

        let cursor: Arc<dyn Writer + 't> = Arc::new(MutCursor::open(self, apath));

        Ok(Some(A::open(cursor, VPath::root())))
    }

    /// Verifies vnode `akey`'s integrity tag over `entry` and its current ACL, so a
    /// writer's own read-back (through [`load_value_at`](Self::load_value_at) and the
    /// mutable accessor) detects a blob altered outside the library rather than
    /// silently returning — or re-sealing over — it. A no-op unless this handle is an
    /// authenticated user holding the integrity key.
    #[cfg(feature = "permissions")]
    fn verify_entry(&self, akey: AKey, entry: &[u8]) -> AdbResult<()> {
        let Principal::User(session) = self.principal.as_ref() else {
            return Ok(());
        };

        let inodes = self.txn.open_table(INODES_TABLE)?;
        let acl = read_acl(&inodes, &self.table, akey)?
            .map(|acl| acl.encode())
            .unwrap_or_default();
        let stored = read_mac(&inodes, &self.table, akey)?;
        let expected = perm::integrity::mac_value(session.key(), &self.table, akey, entry, &acl);

        match stored {
            Some(mac) if perm::integrity::ct_eq(&mac, &expected) => Ok(()),
            _ => Err(AdbError::Tampered(format!(
                "integrity check failed for a vnode in table '{}'",
                self.table
            ))),
        }
    }

    /// Loads the current (uncommitted) value of the file at `path`, or `None` if
    /// absent. Errors if `path` names a directory.
    pub(crate) fn load_value_at(&self, path: &APath) -> AdbResult<Option<Value>> {
        let table = self.txn.open_table(data_def(&self.table))?;

        let Some(akey) = resolve(&table, path)? else {
            return Ok(None);
        };

        let Some(entry) = read_entry(&table, akey)? else {
            return Ok(None);
        };

        #[cfg(feature = "permissions")]
        self.verify_entry(akey, &entry)?;

        let (kind, payload) = entry_split(&entry)?;
        match kind {
            EntryKind::File => Ok(Some(decode(payload)?)),
            EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
        }
    }

    /// Creates the directory at `path` (and any missing ancestors). Idempotent;
    /// errors if a path component is an existing file.
    pub fn mkdir(&self, path: impl IntoArborPath) -> AdbResult<()> {
        let path = path.into_arbor_path()?;
        let mut data = self.txn.open_table(data_def(&self.table))?;

        #[cfg(feature = "entry-timestamps")]
        let mut inodes = self.txn.open_table(INODES_TABLE)?;

        #[cfg(feature = "permissions")]
        let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());
        #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
        let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table);
        #[cfg(not(feature = "entry-timestamps"))]
        let mut ctx = table::Ctx::new(&mut data);

        table::ensure_dir(&mut ctx, &path)?;

        Ok(())
    }

    /// Changes the owner of the file or directory at `path` to the user named
    /// `owner`. Only the master user, a master-group member, or the current owner
    /// may do this (write access alone is not enough).
    #[cfg(feature = "permissions")]
    pub fn chown(&self, path: impl IntoArborPath, owner: &str) -> AdbResult<()> {
        let path = path.into_arbor_path()?;

        let new_uid = {
            let meta = self.txn.open_table(META_TABLE)?;
            perm::store::uid_of(&meta, owner)?
        }
        .ok_or_else(|| AdbError::CannotAccess(format!("no user named '{owner}'")))?;

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::chown_into(&mut ctx, &path, new_uid)
    }

    /// Changes the group of the file or directory at `path` to `group`, or clears
    /// it with `None`.
    #[cfg(feature = "permissions")]
    pub fn chgrp(&self, path: impl IntoArborPath, group: Option<&str>) -> AdbResult<()> {
        let path = path.into_arbor_path()?;

        let new_gid = match group {
            None => None,
            Some(name) => Some(
                {
                    let meta = self.txn.open_table(META_TABLE)?;
                    perm::store::gid_of(&meta, name)?
                }
                .ok_or_else(|| AdbError::CannotAccess(format!("no group named '{name}'")))?,
            ),
        };

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::chgrp_into(&mut ctx, &path, new_gid)
    }

    /// Sets the permission bits of the file or directory at `path`. Requires `write`
    /// on the vnode; the root's ACL cannot be changed.
    #[cfg(feature = "permissions")]
    pub fn chmod(&self, path: impl IntoArborPath, mode: crate::acl::Mode) -> AdbResult<()> {
        let path = path.into_arbor_path()?;
        let bits = mode_to_bits(&mode);

        let mut data = self.txn.open_table(data_def(&self.table))?;
        let mut inodes = self.txn.open_table(INODES_TABLE)?;
        let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());

        table::chmod_into(&mut ctx, &path, bits)
    }

    /// Removes the file or directory at `path` (a directory with its whole
    /// subtree). Returns whether anything was removed.
    pub fn rm(&self, path: impl IntoArborPath) -> AdbResult<bool> {
        let path = path.into_arbor_path()?;
        if path.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot remove the root")));
        }

        self.reindex_around(std::slice::from_ref(&path), |ctx| table::rm_into(ctx, &path))
    }

    /// Moves the vnode at `src` to `dst`, keeping its identity (a relink, not a
    /// copy — so `mv` is O(1) and any accessor holding the vnode's `AKey` stays
    /// valid). Creates `dst`'s parent directories and replaces an existing `dst`.
    /// Errors if `dst` is `src` itself or a descendant of it.
    pub fn mv(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        let src = src.into_arbor_path()?;
        let dst = dst.into_arbor_path()?;

        if dst.names().starts_with(src.names()) {
            return Err(AdbError::CannotAccess(String::from(
                "cannot move a vnode onto itself or into a descendant",
            )));
        }

        if src.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot move the root")));
        }
        if dst.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot move onto the root")));
        }

        // The entity leaves `src` and appears at `dst`, so both scopes are re-indexed.
        let scopes = [src.clone(), dst.clone()];

        self.reindex_around(&scopes, |ctx| table::mv_into(ctx, &src, &dst))
    }

    /// Copies the subtree at `src` to `dst` under fresh identities (a deep copy).
    /// Creates `dst`'s parent directories and replaces an existing `dst`. Errors
    /// if `dst` is `src` itself or a descendant of it.
    pub fn cp(&self, src: impl IntoArborPath, dst: impl IntoArborPath) -> AdbResult<()> {
        let src = src.into_arbor_path()?;
        let dst = dst.into_arbor_path()?;

        if dst.names().starts_with(src.names()) {
            return Err(AdbError::CannotAccess(String::from(
                "cannot copy a vnode onto itself or into a descendant",
            )));
        }

        if dst.is_root() {
            return Err(AdbError::CannotAccess(String::from("cannot copy onto the root")));
        }

        // A copy creates fresh entities at `dst`; `src` is unchanged, so only `dst`
        // needs re-indexing.
        self.reindex_around(std::slice::from_ref(&dst), |ctx| table::cp_into(ctx, &src, &dst))
    }

    /// Loads the table's registered indexes (empty when it has none).
    fn indexes(&self) -> AdbResult<Vec<IndexEntry>> {
        let meta = self.txn.open_table(META_TABLE)?;

        registry::for_table(&meta, &self.table)
    }

    /// Brackets a mutation with index maintenance: remove the affected entities'
    /// current index entries, apply the mutation, then insert the entries the new
    /// state implies. An unindexed table takes the zero-overhead path.
    fn reindex_around<T>(
        &self,
        scopes: &[APath],
        apply: impl FnOnce(&mut table::Ctx<'_, '_>) -> AdbResult<T>,
    ) -> AdbResult<T> {
        let indexes = self.indexes()?;
        let mut data = self.txn.open_table(data_def(&self.table))?;

        #[cfg(feature = "entry-timestamps")]
        let mut inodes = self.txn.open_table(INODES_TABLE)?;

        if indexes.is_empty() {
            #[cfg(feature = "permissions")]
            let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());
            #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
            let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table);
            #[cfg(not(feature = "entry-timestamps"))]
            let mut ctx = table::Ctx::new(&mut data);

            return apply(&mut ctx);
        }

        let mut index = self.txn.open_table(INDEX_TABLE)?;

        for scope in scopes {
            maintenance::delete(&data, &mut index, &indexes, scope)?;
        }

        // Build the write context only around the mutation itself, so index
        // maintenance keeps its shared borrow of the data table before and after.
        let result = {
            #[cfg(feature = "permissions")]
            let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table, self.principal.as_ref());
            #[cfg(all(feature = "entry-timestamps", not(feature = "permissions")))]
            let mut ctx = table::Ctx::new(&mut data, &mut inodes, &self.table);
            #[cfg(not(feature = "entry-timestamps"))]
            let mut ctx = table::Ctx::new(&mut data);

            apply(&mut ctx)?
        };

        for scope in scopes {
            maintenance::insert(&data, &mut index, &indexes, scope)?;
        }

        Ok(result)
    }

    /// A view of this transaction whose access paths are relative to `root`.
    pub fn rooted(&self, root: impl IntoArborPath) -> AdbResult<RootedWrite<'_>> {
        Ok(RootedWrite::new(self, root.into_arbor_path()?))
    }

    /// Commits the transaction, making its changes durable and advancing the
    /// database generation (so cached resolutions from earlier snapshots retire).
    pub fn commit(self) -> AdbResult<()> {
        let WriteTxn {
            txn,
            inner,
            ..
        } = self;

        // Persist any buffered read access times in this same transaction.
        #[cfg(feature = "entry-timestamps")]
        {
            let batch = inner.drain_access_log();
            if !batch.is_empty() {
                let mut inodes = txn.open_table(INODES_TABLE)?;
                for ((table, akey), when) in batch {
                    inode::bump_access(&mut inodes, &table, akey, when)?;
                }
            }
        }

        let guard = inner
            .version_lock()
            .write()
            .map_err(|_| AdbError::CannotAccess(String::from("the version lock was poisoned")))?;

        txn.commit()?;
        inner.bump_generation();
        drop(guard);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Scalar;
    use crate::index::{registry, IndexColumn, IndexDef};
    use crate::ArborDb;

    use redb::ReadableTable; // `index.iter()` in the assertions below

    fn user(age: i64) -> Value {
        Value::Node(BTreeMap::from([(String::from("age"), Value::Leaf(Scalar::I64(age)))]))
    }

    /// The number of physical entries in the shared index table.
    fn index_entries(w: &WriteTxn) -> usize {
        let index = w.txn.open_table(INDEX_TABLE).unwrap();

        index.iter().unwrap().count()
    }

    #[test]
    fn store_and_rm_maintain_a_registered_index() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();
        let w = table.write().unwrap();

        // Register a `users/*` index on the `age` column (raw, via the txn — the
        // public `create_index` lands in a later sub-phase).
        {
            let mut meta = w.txn.open_table(META_TABLE).unwrap();
            let def = IndexDef::new(
                String::from("by_age"),
                String::from("users/*"),
                vec![IndexColumn::asc(VPath::root().child_name("age"))],
                false,
            );

            registry::create(&mut meta, "t", &def).unwrap();
        }

        // Each store under the pattern adds one entry.
        w.store_value("users/alice", &user(30)).unwrap();
        w.store_value("users/bob", &user(40)).unwrap();
        assert_eq!(index_entries(&w), 2);

        // Editing a column rewrites the entity's entry — still one per entity.
        w.store_value("users/alice", &user(31)).unwrap();
        assert_eq!(index_entries(&w), 2);

        // A store outside the pattern is not indexed.
        w.store_value("orgs/acme", &user(99)).unwrap();
        assert_eq!(index_entries(&w), 2);

        // Removing an entity drops its entry.
        w.rm("users/bob").unwrap();
        assert_eq!(index_entries(&w), 1);

        w.commit().unwrap();
    }

    #[test]
    fn patch_scalar_replaces_a_same_width_leaf_in_place() {
        let mut entry = file_entry(&encode(&user(30)));
        let len_before = entry.len();
        let age = VPath::root().child_name("age");

        // I64 → I64 keeps the width, so the blob is patched without changing length.
        assert!(table::patch_scalar(&mut entry, &age, &Scalar::I64(31)).unwrap());
        assert_eq!(entry.len(), len_before);

        let (kind, payload) = entry_split(&entry).unwrap();
        assert_eq!(kind, EntryKind::File);
        assert_eq!(decode(payload).unwrap(), user(31));
    }

    #[test]
    fn patch_scalar_declines_a_width_change_or_non_leaf() {
        let mut entry = file_entry(&encode(&user(30)));

        // A width change (I64 → Str), a non-leaf path (the object root), and an absent
        // field all decline, leaving the caller to re-encode.
        let age = VPath::root().child_name("age");
        assert!(!table::patch_scalar(&mut entry, &age, &Scalar::Str(String::from("thirty"))).unwrap());
        assert!(!table::patch_scalar(&mut entry, &VPath::root(), &Scalar::I64(1)).unwrap());

        let missing = VPath::root().child_name("missing");
        assert!(!table::patch_scalar(&mut entry, &missing, &Scalar::I64(1)).unwrap());

        // A declined patch leaves the entry byte-for-byte unchanged.
        let (_, payload) = entry_split(&entry).unwrap();
        assert_eq!(decode(payload).unwrap(), user(30));
    }

    #[test]
    fn an_in_place_scalar_edit_maintains_a_registered_index() {
        let db = ArborDb::create_in_memory().unwrap();
        let table = db.open_table("t").unwrap();

        {
            let w = table.write().unwrap();

            {
                let mut meta = w.txn.open_table(META_TABLE).unwrap();
                let def = IndexDef::new(
                    String::from("by_age"),
                    String::from("users/*"),
                    vec![IndexColumn::asc(VPath::root().child_name("age"))],
                    false,
                );

                registry::create(&mut meta, "t", &def).unwrap();
            }

            w.store_value("users/alice", &user(30)).unwrap();
            assert_eq!(index_entries(&w), 1);

            // An in-place edit of the indexed column keeps exactly one entry: the old
            // key is removed and the new one inserted around the patch.
            let age = VPath::root().child_name("age");
            w.put_scalar_at(&APath::parse("users/alice").unwrap(), &age, Scalar::I64(31))
                .unwrap();
            assert_eq!(index_entries(&w), 1);

            w.commit().unwrap();
        }

        // The edit persisted.
        let r = table.read().unwrap();
        assert_eq!(r.get_as::<i64>("users/alice", "age").unwrap(), Some(31));
    }
}
