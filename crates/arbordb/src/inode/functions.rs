use super::{node::Inode, datetime::NodeTimestamps, underlying_timestamps::UnderlyingTimestamps, InodeTable};
use crate::{codec::put_bytes, error::AdbResult, AKey};
use redb::ReadableTable;

#[cfg(feature = "permissions")]
use super::acl::Acl;

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
pub(crate) fn read_times<R>(inodes: &R, table: &str, akey: AKey) -> AdbResult<Option<NodeTimestamps>>
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

    inode.set_acl(Acl::new(owner, None, Acl::default_mode()));
    inodes.insert(key.as_slice(), inode.encode().as_slice())?;

    Ok(())
}

/// Replaces vnode `akey`'s ACL wholesale (used by chown/chgrp/chmod).
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

/// Reads vnode `akey`'s integrity tag, or `None` if it has none yet.
#[cfg(feature = "permissions")]
pub(crate) fn read_mac<R>(inodes: &R, table: &str, akey: AKey) -> AdbResult<Option<[u8; 32]>>
where
    R: ReadableTable<&'static [u8], &'static [u8]>, {
    let Some(guard) = inodes.get(inode_key(table, akey).as_slice())? else {
        return Ok(None);
    };

    Ok(Inode::decode(guard.value())?.mac())
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

/// Clears the group of every inode whose ACL names `gid` (used when a group is
/// deleted). Iterates the whole table, so it is O(number of vnodes with metadata).
#[cfg(feature = "permissions")]
pub(crate) fn strip_group(inodes: &mut InodeTable, gid: u32) -> AdbResult<()> {
    let mut updates: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    for entry in inodes.iter()? {
        let (key, value) = entry?;
        let mut inode = Inode::decode(value.value())?;

        if let Some(mut acl) = inode.acl()
            && acl.group() == Some(gid)
        {
            acl.set_group(None);
            inode.set_acl(acl);
            updates.push((key.value().to_vec(), inode.encode()));
        }
    }

    for (key, value) in updates {
        inodes.insert(key.as_slice(), value.as_slice())?;
    }

    Ok(())
}
