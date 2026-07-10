use super::ctx::Context;
use crate::{
    codec::{decode, encode, encode_dir, ArchivedDir, ArchivedValue},
    data::Scalar,
    engine::{dir_entry, file_entry, entry_split, EntryKind},
    error::{AdbError, AdbResult},
    path::{APath, VPath},
    value::Value,
    AKey,
};

use std::collections::BTreeMap;

#[cfg(feature = "permissions")]
use crate::{acl::Rights, inode::Acl, perm};

/// Stores the pre-encoded `value_blob` as a file at `path` (a non-root path),
/// creating parents and replacing whatever was there. Shared tail behind both
/// [`store_value_into`] (which encodes a [`Value`] first) and the direct Serde path.
pub(in crate::txn) fn store_blob_into(ctx: &mut Context, path: &APath, value_blob: &[u8]) -> AdbResult<()> {
    let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");
    let parent = ensure_dir(ctx, &parent_path)?;

    #[cfg(feature = "permissions")]
    ctx.check(parent, Rights::Access)?;

    let akey = match ctx.child(parent, name)? {
        Some(node) => {
            if ctx.kind(node)? == Some(EntryKind::File) {
                #[cfg(feature = "permissions")]
                ctx.check(node, Rights::Modify)?;

                node
            } else {
                // Replacing a directory with a file deletes the directory's
                // subtree, so it must be deletable, and linking the new file
                // modifies the parent.
                #[cfg(feature = "permissions")]
                {
                    ctx.check_deletable(node)?;
                    ctx.check(parent, Rights::Modify)?;
                }

                cascade_delete(ctx, node)?;
                let fresh = AKey::generate();
                link_child(ctx, parent, name, fresh)?;
                fresh
            }
        }
        None => {
            #[cfg(feature = "permissions")]
            ctx.check(parent, Rights::Modify)?;

            let fresh = AKey::generate();
            link_child(ctx, parent, name, fresh)?;
            fresh
        }
    };

    let entry = file_entry(value_blob);
    ctx.put_entry(akey, entry.as_slice())?;

    Ok(())
}

/// Stores `value` as a file at `path`, encoding it to a blob first.
pub(in crate::txn) fn store_value_into(ctx: &mut Context, path: &APath, value: &Value) -> AdbResult<()> {
    store_blob_into(ctx, path, &encode(value))
}

/// Sets the scalar at `at` inside the file at `path`. When the new scalar keeps the
/// current leaf's byte width the blob is patched in place — no decode, no
/// re-encode; otherwise the value is decoded, updated, and re-encoded. Creates the
/// file (and its parents) when it does not exist yet.
pub(in crate::txn) fn put_scalar_into(ctx: &mut Context, path: &APath, at: &VPath, scalar: &Scalar) -> AdbResult<()> {
    let akey = ctx.resolve(path)?;
    let entry = match akey {
        Some(akey) => ctx.read_verified(akey)?,
        None => None,
    };

    let mut value = match (akey, entry) {
        (Some(akey), Some(mut entry)) => {
            #[cfg(feature = "permissions")]
            ctx.check(akey, Rights::Modify)?;

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
pub(in crate::txn) fn patch_scalar(entry: &mut [u8], at: &VPath, scalar: &Scalar) -> AdbResult<bool> {
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
pub(in crate::txn) fn rm_into(ctx: &mut Context, path: &APath) -> AdbResult<bool> {
    let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");

    let Some(parent) = ctx.resolve(&parent_path)? else {
        return Ok(false);
    };

    #[cfg(feature = "permissions")]
    ctx.check(parent, Rights::Access)?;

    let Some(akey) = ctx.child(parent, name)? else {
        return Ok(false);
    };

    // Deleting a vnode needs `Delete` on it (and, for a directory, on every
    // descendant the cascade removes). Holding it also authorizes unlinking the
    // name from the parent.
    #[cfg(feature = "permissions")]
    ctx.check_deletable(akey)?;

    cascade_delete(ctx, akey)?;
    unlink_child(ctx, parent, name)?;

    Ok(true)
}

/// Relinks the vnode at `src` to `dst`, keeping its identity.
pub(in crate::txn) fn mv_into(ctx: &mut Context, src: &APath, dst: &APath) -> AdbResult<()> {
    let (src_parent_path, src_name) = src.split_last().expect("a non-root path has a parent and a name");
    let (dst_parent_path, dst_name) = dst.split_last().expect("a non-root path has a parent and a name");

    let Some(src_parent) = ctx.resolve(&src_parent_path)? else {
        return Err(AdbError::ValueNotFound(src.clone()));
    };

    #[cfg(feature = "permissions")]
    ctx.check(src_parent, Rights::Access)?;

    let Some(akey) = ctx.child(src_parent, src_name)? else {
        return Err(AdbError::ValueNotFound(src.clone()));
    };

    // A move relinks the source elsewhere: it leaves its old name (a delete on
    // the source, which also authorizes unlinking it from its parent) and
    // appears under a new one (a modify of the destination parent).
    #[cfg(feature = "permissions")]
    ctx.check(akey, Rights::Delete)?;

    let dst_parent = ensure_dir(ctx, &dst_parent_path)?;

    #[cfg(feature = "permissions")]
    ctx.check(dst_parent, Rights::Modify)?;

    if let Some(existing) = ctx.child(dst_parent, dst_name)? {
        #[cfg(feature = "permissions")]
        ctx.check_deletable(existing)?;

        cascade_delete(ctx, existing)?;
    }

    link_child(ctx, dst_parent, dst_name, akey)?;
    unlink_child(ctx, src_parent, src_name)?;

    Ok(())
}

/// Deep-copies the subtree at `src` to `dst` under fresh identities.
pub(in crate::txn) fn cp_into(ctx: &mut Context, src: &APath, dst: &APath) -> AdbResult<()> {
    let (dst_parent_path, dst_name) = dst.split_last().expect("a non-root path has a parent and a name");

    let Some(src_akey) = ctx.resolve(src)? else {
        return Err(AdbError::ValueNotFound(src.clone()));
    };

    let dst_parent = ensure_dir(ctx, &dst_parent_path)?;

    #[cfg(feature = "permissions")]
    ctx.check(dst_parent, Rights::Modify)?;

    if let Some(existing) = ctx.child(dst_parent, dst_name)? {
        #[cfg(feature = "permissions")]
        ctx.check_deletable(existing)?;

        cascade_delete(ctx, existing)?;
    }

    let copy = deep_copy(ctx, src_akey)?;
    link_child(ctx, dst_parent, dst_name, copy)?;

    Ok(())
}

/// Writes `map` as directory `akey`'s children.
pub(in crate::txn) fn put_dir(ctx: &mut Context, akey: AKey, map: &BTreeMap<String, AKey>) -> AdbResult<()> {
    let entry = dir_entry(&encode_dir(map));
    ctx.put_entry(akey, entry.as_slice())?;

    Ok(())
}

/// Adds (or replaces) a `name → child` link in directory `parent`.
pub(in crate::txn) fn link_child(ctx: &mut Context, parent: AKey, name: &str, child: AKey) -> AdbResult<()> {
    let mut map = ctx.dir_children(parent)?;
    map.insert(name.to_string(), child);

    put_dir(ctx, parent, &map)
}

/// Removes the `name` link from directory `parent`.
pub(in crate::txn) fn unlink_child(ctx: &mut Context, parent: AKey, name: &str) -> AdbResult<()> {
    let mut map = ctx.dir_children(parent)?;
    map.remove(name);

    put_dir(ctx, parent, &map)
}

/// Ensures the root directory exists.
pub(in crate::txn) fn ensure_root(ctx: &mut Context) -> AdbResult<()> {
    if ctx.read_verified(AKey::ROOT)?.is_none() {
        put_dir(ctx, AKey::ROOT, &BTreeMap::new())?;
    }

    Ok(())
}

/// Ensures every directory along `path` exists, returning the deepest one's key.
pub(in crate::txn) fn ensure_dir(ctx: &mut Context, path: &APath) -> AdbResult<AKey> {
    ensure_root(ctx)?;

    let mut akey = AKey::ROOT;
    for name in path.names() {
        #[cfg(feature = "permissions")]
        ctx.check(akey, Rights::Access)?;

        match ctx.child(akey, name.as_str())? {
            Some(child) => {
                if ctx.kind(child)? != Some(EntryKind::Dir) {
                    return Err(AdbError::CannotAccess(format!("'{name}' is a file, not a directory")));
                }

                akey = child;
            }
            None => {
                #[cfg(feature = "permissions")]
                ctx.check(akey, Rights::Modify)?;

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
pub(in crate::txn) fn cascade_delete(ctx: &mut Context, akey: AKey) -> AdbResult<()> {
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
pub(in crate::txn) fn deep_copy(ctx: &mut Context, akey: AKey) -> AdbResult<AKey> {
    #[cfg(feature = "permissions")]
    ctx.check(akey, Rights::Access)?;

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
pub(in crate::txn) fn chown_into(ctx: &mut Context, path: &APath, new_uid: u32) -> AdbResult<()> {
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

    perm::access::authorize_chown(ctx.principal(), &acl)?;
    acl.set_owner_uid(new_uid);

    ctx.write_acl(akey, acl)?;
    ctx.reseal_integrity(akey)
}

/// Mutates the ACL of the vnode at `path` through `apply`, then re-seals its
/// integrity tags. Requires `Modify` on the vnode; the root ACL is immutable.
/// This is the one path behind `set_acl` / `add_group` / `del_group`.
#[cfg(feature = "permissions")]
pub(in crate::txn) fn set_acl_into(ctx: &mut Context, path: &APath, apply: impl FnOnce(&mut Acl)) -> AdbResult<()> {
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

    perm::access::authorize(ctx.principal(), akey, Some(&acl), Rights::Modify)?;
    apply(&mut acl);

    ctx.write_acl(akey, acl)?;
    ctx.reseal_integrity(akey)
}

/// Recursively collects the `(parent, name)` of every top-most vnode owned by
/// `uid` under directory `dir`. An owned directory is collected whole (and not
/// descended into); a non-owned directory is descended to find owned vnodes.
#[cfg(feature = "permissions")]
pub(in crate::txn) fn collect_owned(
    ctx: &Context,
    dir: AKey,
    uid: u32,
    out: &mut Vec<(AKey, String)>,
) -> AdbResult<()> {
    for (name, child) in ctx.dir_children(dir)? {
        if ctx.acl(child)?.map(|acl| acl.owner_uid()) == Some(uid) {
            out.push((dir, name));
        } else if ctx.kind(child)? == Some(EntryKind::Dir) {
            collect_owned(ctx, child, uid, out)?;
        }
    }

    Ok(())
}
