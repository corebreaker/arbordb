//! The read/modify/write operations over the `$inodes` table: keying, timestamp
//! touches and access bumps, and (with `permissions`) ACL and integrity-tag
//! accessors. Every mutation decodes the whole inode, changes one section, and
//! re-encodes it, so no operation ever drops another feature's metadata.

use super::{node::Inode, datetime::NodeTimestamps, underlying_timestamps::UnderlyingTimestamps, InodeTable};
use crate::{codec::put_bytes, error::AdbResult, AKey};
use redb::ReadableTable;

#[cfg(feature = "permissions")]
use super::acl::Acl;

#[cfg(feature = "permissions")]
use crate::crypto::SIG_LEN;

/// Encodes the composite `$inodes` key for vnode `akey` in table `table`
/// (length-prefixed name, then the 16-byte key). Lookups are point-only, so key
/// ordering is irrelevant.
pub(super) fn inode_key(table: &str, akey: AKey) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + table.len() + 16);
    put_bytes(&mut key, table.as_bytes());
    key.extend_from_slice(&akey.into_bytes());

    key
}

/// Records that vnode `akey` in `table` was just written at `now`: sets `modified`
/// (and, on first sight, `created` and `accessed`) while preserving every other
/// section.
pub(crate) fn touch(inodes: &mut InodeTable, table: &str, akey: AKey, now: i64) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let mut inode = match inodes.get(key.as_slice())? {
        Some(guard) => Inode::decode(guard.value())?,
        None => Inode::default(),
    };

    let (created, accessed) = match inode.timestamps() {
        Some(times) => (times.created(), times.accessed()),
        None => (now, now),
    };

    inode.set_timestamps(UnderlyingTimestamps::new(created, now, accessed));
    inodes.insert(key.as_slice(), inode.encode().as_slice())?;
    Ok(())
}

/// Records a write of vnode `akey` **and** seals its integrity tags in a single
/// read-modify-write of the inode — the protected write hot path.
///
/// It sets `modified` to `now` (and, on first sight, `created`/`accessed`); stamps
/// the default ACL owned by `owner` when the vnode has none yet and `owner` is
/// `Some` (a non-root vnode written by an authenticated user, so an overwrite keeps
/// the existing ACL); then stores the MAC and signature `seal` computes over the
/// vnode's now-current ACL bytes. Every section is written back at once, so the
/// `$inodes` entry is decoded once and inserted once, instead of the five reads and
/// three writes the separate `touch` / default-ACL / `seal_mac` / `seal_sig` steps
/// used to cost.
#[cfg(feature = "permissions")]
pub(crate) fn stamp_and_seal(
    inodes: &mut InodeTable,
    table: &str,
    akey: AKey,
    now: i64,
    owner: Option<u32>,
    seal: impl FnOnce(&[u8]) -> ([u8; 32], [u8; SIG_LEN]),
) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let mut inode = match inodes.get(key.as_slice())? {
        Some(guard) => Inode::decode(guard.value())?,
        None => Inode::default(),
    };

    let (created, accessed) = match inode.timestamps() {
        Some(times) => (times.created(), times.accessed()),
        None => (now, now),
    };

    inode.set_timestamps(UnderlyingTimestamps::new(created, now, accessed));

    // A fresh non-root vnode gets its owner's default ACL; an overwrite keeps the
    // ACL already there (so re-storing a file never resets its permissions).
    if let Some(owner) = owner
        && inode.acl().is_none()
    {
        inode.set_acl(Acl::default_for(owner));
    }

    // Both tags bind the (now current) ACL bytes, so seal after the ACL is settled.
    let acl = inode.acl().map(|acl| acl.encode()).unwrap_or_default();
    let (mac, sig) = seal(&acl);
    inode.set_mac(mac);
    inode.set_sig(sig);

    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Drops vnode `akey`'s inode (called when the vnode itself is removed).
pub(crate) fn forget(inodes: &mut InodeTable, table: &str, akey: AKey) -> AdbResult<()> {
    inodes.remove(inode_key(table, akey).as_slice())?;

    Ok(())
}

/// Advances vnode `akey`'s access time to `when` (never backwards), leaving every
/// other section intact. A vnode with no inode yet is left untouched — a deferred
/// access flush never resurrects a deleted vnode.
pub(crate) fn bump_access(inodes: &mut InodeTable, table: &str, akey: AKey, when: i64) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let Some(guard) = inodes.get(key.as_slice())? else {
        return Ok(());
    };

    let mut inode = Inode::decode(guard.value())?;
    drop(guard);

    match inode.timestamps_mut() {
        Some(times) if when > times.accessed() => {
            times.set_accessed(when);
        }
        Some(_) => {
            return Ok(());
        }
        None => {
            inode.set_timestamps(UnderlyingTimestamps::new(when, when, when));
        }
    }

    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Reads vnode `akey`'s timestamps, or `None` if it has no inode yet.
pub(crate) fn read_timestamps<R>(inodes: &R, table: &str, akey: AKey) -> AdbResult<Option<NodeTimestamps>>
where
    R: ReadableTable<&'static [u8], &'static [u8]>, {
    let Some(guard) = inodes.get(inode_key(table, akey).as_slice())? else {
        return Ok(None);
    };

    match Inode::decode(guard.value())?.timestamps() {
        Some(times) => Ok(Some(times.into_node_times()?)),
        None => Ok(None),
    }
}

/// Reads vnode `akey`'s ACL, or `None` if it has none yet.
#[cfg(feature = "permissions")]
pub(crate) fn read_acl<R>(inodes: &R, table: &str, akey: AKey) -> AdbResult<Option<Acl>>
where
    R: ReadableTable<&'static [u8], &'static [u8]>, {
    let Some(guard) = inodes.get(inode_key(table, akey).as_slice())? else {
        return Ok(None);
    };

    Ok(Inode::decode(guard.value())?.acl())
}

/// A vnode's ACL and both integrity tags, decoded together by [`read_meta`]: the
/// ACL authorizes access, and one of the two tags (keyed MAC for a user, signature
/// for a guest) verifies the value.
#[cfg(feature = "permissions")]
pub(crate) type Meta = (Option<Acl>, Option<[u8; 32]>, Option<[u8; SIG_LEN]>);

/// Reads vnode `akey`'s ACL and both integrity tags in a **single** decode of the
/// inode — the read/verify hot path, where authorization needs the ACL and
/// verification needs the ACL plus one of the tags. Returns all-`None` if the vnode
/// has no inode yet.
#[cfg(feature = "permissions")]
pub(crate) fn read_meta<R>(inodes: &R, table: &str, akey: AKey) -> AdbResult<Meta>
where
    R: ReadableTable<&'static [u8], &'static [u8]>, {
    let Some(guard) = inodes.get(inode_key(table, akey).as_slice())? else {
        return Ok((None, None, None));
    };

    let inode = Inode::decode(guard.value())?;

    Ok((inode.acl(), inode.mac(), inode.sig()))
}

/// Reads vnode `akey`'s raw inode bytes (an owned copy), or `None` if it has no
/// inode yet. Feeds the read-side inode cache, which stores the undecoded blob
/// (principal-independent, so it is safe to share) and decodes it per read — a
/// decode that allocates nothing for the common default ACL.
#[cfg(feature = "permissions")]
pub(crate) fn read_inode_bytes<R>(inodes: &R, table: &str, akey: AKey) -> AdbResult<Option<Vec<u8>>>
where
    R: ReadableTable<&'static [u8], &'static [u8]>, {
    Ok(inodes
        .get(inode_key(table, akey).as_slice())?
        .map(|guard| guard.value().to_vec()))
}

/// Decodes vnode metadata (ACL + both integrity tags) from already-fetched inode
/// bytes — the cached-read counterpart of [`read_meta`].
#[cfg(feature = "permissions")]
pub(crate) fn decode_meta(bytes: &[u8]) -> AdbResult<Meta> {
    let inode = Inode::decode(bytes)?;

    Ok((inode.acl(), inode.mac(), inode.sig()))
}

/// Stamps the default ACL on vnode `akey` — `owner`, `group`, and the default
/// mode — but only when it has none yet, so an overwrite preserves the ACL.
#[cfg(feature = "permissions")]
pub(crate) fn set_default_acl(inodes: &mut InodeTable, table: &str, akey: AKey, owner: u32) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let mut inode = match inodes.get(key.as_slice())? {
        Some(guard) => Inode::decode(guard.value())?,
        None => Inode::default(),
    };

    if inode.acl().is_some() {
        return Ok(());
    }

    inode.set_acl(Acl::default_for(owner));
    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Replaces vnode `akey`'s ACL wholesale (used by chown/set_acl).
#[cfg(feature = "permissions")]
pub(crate) fn set_acl(inodes: &mut InodeTable, table: &str, akey: AKey, acl: Acl) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let mut inode = match inodes.get(key.as_slice())? {
        Some(guard) => Inode::decode(guard.value())?,
        None => Inode::default(),
    };

    inode.set_acl(acl);
    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Stores vnode `akey`'s integrity tag, preserving every other section.
#[cfg(feature = "permissions")]
pub(crate) fn seal_mac(inodes: &mut InodeTable, table: &str, akey: AKey, mac: [u8; 32]) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let mut inode = match inodes.get(key.as_slice())? {
        Some(guard) => Inode::decode(guard.value())?,
        None => Inode::default(),
    };

    inode.set_mac(mac);
    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Stores vnode `akey`'s value signature, preserving every other section.
#[cfg(feature = "permissions")]
pub(crate) fn seal_sig(
    inodes: &mut InodeTable,
    table: &str,
    akey: AKey,
    sig: [u8; crate::crypto::SIG_LEN],
) -> AdbResult<()> {
    let key = inode_key(table, akey);

    let mut inode = match inodes.get(key.as_slice())? {
        Some(guard) => Inode::decode(guard.value())?,
        None => Inode::default(),
    };

    inode.set_sig(sig);
    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Clears the group of every inode whose ACL names `gid` (used when a group is
/// deleted). Iterates the whole table, so it is O(number of vnodes with metadata).
#[cfg(feature = "permissions")]
pub(crate) fn strip_group(inodes: &mut InodeTable, gid: u32) -> AdbResult<()> {
    let mut updates: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    for entry in inodes.iter()? {
        let (key, value) = entry?;
        let mut inode = Inode::decode(value.value())?;

        if let Some(mut acl) = inode.acl()
            && acl.has_group(gid)
        {
            acl.remove_group(gid);
            inode.set_acl(acl);
            updates.push((key.value().to_vec(), inode.encode()));
        }
    }

    for (key, value) in updates {
        inodes.insert(key.as_slice(), value.as_slice())?;
    }

    Ok(())
}
