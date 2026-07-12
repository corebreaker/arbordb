use super::ctx::Context;
use crate::{
    codec::{decode, encode, encode_dir, ArchivedValue},
    data::Scalar,
    engine::{dir_entry, file_entry, entry_split, EntryKind},
    error::{AdbError, AdbResult},
    path::{APath, VPath},
    value::Value,
    AKey,
};

use smol_str::SmolStr;
use std::collections::{BTreeMap, HashMap};

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

/// Resolves `path` and reads its (verified) entry — the walk-the-tree path shared by
/// [`resolve_target`] both as its default and as the fallback when a stale hint no
/// longer names a live a-node.
fn resolve_and_read(ctx: &Context, path: &APath) -> AdbResult<(Option<AKey>, Option<Vec<u8>>)> {
    let akey = ctx.resolve(path)?;
    let entry = match akey {
        Some(akey) => ctx.read_verified(akey)?,
        None => None,
    };

    Ok((akey, entry))
}

/// Resolves the target file for a scalar write, honouring a caller's key `hint`
/// where that is sound.
///
/// The `hint` is an a-node key a mutable accessor resolved for `path`, paired with the
/// transaction's structural-change count at that moment. Without access control it is
/// trusted only while that count is unchanged: a path resolves through `name → child`
/// links alone, so an unchanged count means `path` still resolves to the hinted a-node
/// and the directory tree need not be re-walked. An interleaved `mv`/`rm`/`store` bumps
/// the count — since an `AKey` is stable across renames a moved a-node stays live, but
/// `path` no longer resolves to it — so the hint is dropped and the path re-resolved
/// afresh (recreating the file if `path` is now gone), never silently patching an
/// a-node the hint's identity was relinked to elsewhere.
///
/// Under `permissions`, `ctx.resolve` performs the per-directory `Access` checks a
/// scalar edit still requires, so the hint is ignored there and the path is always
/// resolved afresh.
pub(super) fn resolve_target(
    ctx: &Context,
    path: &APath,
    hint: Option<(AKey, u64)>,
) -> AdbResult<(Option<AKey>, Option<Vec<u8>>)> {
    #[cfg(not(feature = "permissions"))]
    if let Some((key, epoch)) = hint
        && epoch == ctx.structure_epoch()
        && let Some(entry) = ctx.read_verified(key)?
    {
        return Ok((Some(key), Some(entry)));
    }

    #[cfg(feature = "permissions")]
    let _ = hint;

    resolve_and_read(ctx, path)
}

/// Sets the scalar at `at` inside the file at `path`. When the new scalar keeps the
/// current leaf's byte width the blob is patched in place — no decode, no
/// re-encode; otherwise the value is decoded, updated, and re-encoded. Creates the
/// file (and its parents) when it does not exist yet.
///
/// `hint` is an a-node key the caller already resolved for `path` (a mutable accessor
/// that just walked it), paired with the transaction's structural-change count at that
/// moment, passed on to [`resolve_target`] to skip re-walking the directory tree while
/// that count is unchanged.
pub(in crate::txn) fn put_scalar_into(
    ctx: &mut Context,
    path: &APath,
    hint: Option<(AKey, u64)>,
    at: &VPath,
    scalar: &Scalar,
) -> AdbResult<()> {
    let (akey, entry) = resolve_target(ctx, path, hint)?;

    // An existing file: edit its blob and buffer the result (written and sealed once at
    // commit through `put_file_edit`), so a burst of edits to one value coalesces into
    // a single re-encode and — under `permissions` — a single signature.
    if let (Some(akey), Some(mut entry)) = (akey, entry) {
        #[cfg(feature = "permissions")]
        ctx.check(akey, Rights::Modify)?;

        // Fast path: a leaf that keeps its width is patched in place — every other
        // offset in the blob stays valid, so nothing is re-encoded.
        if patch_scalar(&mut entry, at, scalar)? {
            return ctx.put_file_edit(akey, entry);
        }

        // Slow path: decode the (untouched) blob, edit the value, re-encode.
        let (kind, payload) = entry_split(&entry)?;
        let mut value = match kind {
            EntryKind::File => decode(payload)?,
            EntryKind::Dir => {
                return Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file")));
            }
        };

        value.set_value(at, Value::Leaf(scalar.clone()));

        return ctx.put_file_edit(akey, file_entry(&encode(&value)));
    }

    // Nothing there yet: create the file (writing through, so its ACL lands at once).
    let mut value = Value::default();
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

/// Removes the a-node at `path` (a non-root path) and its subtree. Returns whether
/// anything was removed.
pub(in crate::txn) fn rm_into(ctx: &mut Context, path: &APath) -> AdbResult<bool> {
    let (parent_path, name) = path.split_last().expect("a non-root path has a parent and a name");

    let Some(parent) = ctx.resolve(&parent_path)? else {
        return Ok(false);
    };

    #[cfg(feature = "permissions")]
    ctx.check(parent, Rights::Access)?;

    // A borrowed child lookup — `None` (nothing to remove) when `parent` is absent,
    // a file, or has no such child, so `rm` stays idempotent on a missing path.
    let Some(akey) = ctx.child(parent, name)? else {
        return Ok(false);
    };

    // Deleting an a-node needs `Delete` on it (and, for a directory, on every
    // descendant the cascade removes). Holding it also authorizes unlinking the
    // name from the parent.
    #[cfg(feature = "permissions")]
    ctx.check_deletable(akey)?;

    cascade_delete(ctx, akey)?;
    unlink_child(ctx, parent, name)?;

    Ok(true)
}

/// Relinks the a-node at `src` to `dst`, keeping its identity.
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

/// Writes `map` as directory `akey`'s children — buffered into the write-back cache
/// when buffering is on, otherwise encoded and written to the engine at once.
pub(in crate::txn) fn put_dir(ctx: &mut Context, akey: AKey, map: &BTreeMap<SmolStr, AKey>) -> AdbResult<()> {
    if ctx.buffering() {
        // A buffered directory's blob — and, under `permissions`, its integrity seal —
        // is written once at the commit-time flush. Stamp a fresh directory's ACL
        // eagerly, though, so an access check later in this same transaction sees it.
        #[cfg(feature = "permissions")]
        ctx.stamp_new_dir_acl(akey)?;

        ctx.buffer_dir(akey, map.clone());

        return Ok(());
    }

    let entry = dir_entry(&encode_dir(map));
    ctx.put_entry(akey, entry.as_slice())?;

    Ok(())
}

/// Flushes the write-back cache: encodes each buffered directory's child-map and writes
/// both the directories and the edited files through [`Context::put_entry`] — which,
/// under `permissions`, seals each blob's integrity tags. This is the O(N) tail of a
/// bulk load and the once-per-value tail of a burst of edits, run at commit (or when
/// index maintenance forces the engine current mid-transaction).
pub(in crate::txn) fn flush_buffered(
    ctx: &mut Context,
    dirs: HashMap<AKey, BTreeMap<SmolStr, AKey>>,
    files: HashMap<AKey, Vec<u8>>,
) -> AdbResult<()> {
    for (akey, map) in dirs {
        let entry = dir_entry(&encode_dir(&map));
        ctx.put_entry(akey, entry.as_slice())?;
    }

    // A buffered file entry is already the full tagged blob; write (and seal) it as is.
    for (akey, entry) in files {
        ctx.put_entry(akey, entry.as_slice())?;
    }

    Ok(())
}

/// Adds (or replaces) a `name → child` link in directory `parent`. When buffering,
/// the parent's cached child-map is mutated in place (O(1)); otherwise the whole
/// directory is read, edited, and rewritten.
pub(in crate::txn) fn link_child(ctx: &mut Context, parent: AKey, name: &str, child: AKey) -> AdbResult<()> {
    // A relink repoints which a-node a path resolves to, so bump the structural-change
    // count that invalidates a mutable cursor's stale key hint (see `resolve_target`).
    ctx.bump_structure();

    if ctx.buffering() {
        return ctx.dir_link(parent, name, child);
    }

    let mut map = ctx.dir_children(parent)?;
    map.insert(SmolStr::from(name), child);

    put_dir(ctx, parent, &map)
}

/// Removes the `name` link from directory `parent` (in place when buffering).
pub(in crate::txn) fn unlink_child(ctx: &mut Context, parent: AKey, name: &str) -> AdbResult<()> {
    // A relink repoints which a-node a path resolves to, so bump the structural-change
    // count that invalidates a mutable cursor's stale key hint (see `resolve_target`).
    ctx.bump_structure();

    if ctx.buffering() {
        return ctx.dir_unlink(parent, name);
    }

    let mut map = ctx.dir_children(parent)?;
    map.remove(name);

    put_dir(ctx, parent, &map)
}

/// Ensures the root directory exists.
pub(in crate::txn) fn ensure_root(ctx: &mut Context) -> AdbResult<()> {
    // A bare presence probe — the root's contents are read (and verified) by the
    // directory walk that follows, so materializing its blob here would be wasted.
    if !ctx.has_entry(AKey::ROOT)? {
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

/// Removes a-node `akey` and, if it is a directory, its whole subtree.
pub(in crate::txn) fn cascade_delete(ctx: &mut Context, akey: AKey) -> AdbResult<()> {
    // Read the a-node borrowed once: a file yields no children (no owned copy of its
    // blob), a directory yields the subtree to recurse into.
    let Some(children) = ctx.cascade_children(akey)? else {
        return Ok(());
    };

    for child in children {
        cascade_delete(ctx, child)?;
    }

    ctx.remove_entry(akey)?;

    Ok(())
}

/// Deep-copies a-node `akey` (a file's blob verbatim, a directory recursively) under
/// a freshly generated key, returning that key.
pub(in crate::txn) fn deep_copy(ctx: &mut Context, akey: AKey) -> AdbResult<AKey> {
    #[cfg(feature = "permissions")]
    ctx.check(akey, Rights::Access)?;

    let fresh = AKey::generate();

    // `kind` and `dir_children` consult the write-back cache, so a directory this
    // transaction has already modified is copied in its current (buffered) state, not
    // the stale engine one. A file's blob is always in the engine (files write through).
    match ctx
        .kind(akey)?
        .ok_or_else(|| AdbError::Corrupt("copying a missing a-node".into()))?
    {
        EntryKind::File => {
            let entry = ctx
                .read_verified(akey)?
                .ok_or_else(|| AdbError::Corrupt("copying a missing a-node".into()))?;

            ctx.put_entry(fresh, entry.as_slice())?;
        }
        EntryKind::Dir => {
            let mut copied = BTreeMap::new();
            for (name, child) in ctx.dir_children(akey)? {
                copied.insert(name, deep_copy(ctx, child)?);
            }

            put_dir(ctx, fresh, &copied)?;
        }
    }

    Ok(fresh)
}

/// Changes the owner of the a-node at `path` to `new_uid`.
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
        .ok_or_else(|| AdbError::CannotAccess(String::from("this a-node has no ACL")))?;

    perm::access::authorize_chown(ctx.principal(), &acl)?;
    acl.set_owner_uid(new_uid);

    ctx.write_acl(akey, acl)?;
    ctx.reseal_integrity(akey)
}

/// Mutates the ACL of the a-node at `path` through `apply`, then re-seals its
/// integrity tags. Requires `Modify` on the a-node; the root ACL is immutable.
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
        .ok_or_else(|| AdbError::CannotAccess(String::from("this a-node has no ACL")))?;

    perm::access::authorize(ctx.principal(), akey, Some(&acl), Rights::Modify)?;
    apply(&mut acl);

    ctx.write_acl(akey, acl)?;
    ctx.reseal_integrity(akey)
}

/// Recursively collects the `(parent, name)` of every top-most a-node owned by
/// `uid` under directory `dir`. An owned directory is collected whole (and not
/// descended into); a non-owned directory is descended to find owned a-nodes.
#[cfg(feature = "permissions")]
pub(in crate::txn) fn collect_owned(
    ctx: &Context,
    dir: AKey,
    uid: u32,
    out: &mut Vec<(AKey, SmolStr)>,
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
