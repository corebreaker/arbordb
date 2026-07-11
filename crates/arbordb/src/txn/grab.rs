//! The read surface shared by [`ReadTxn`](super::ReadTxn) and
//! [`WriteTxn`](super::WriteTxn).
//!
//! A read transaction reads a committed snapshot; a write transaction reads its
//! own **uncommitted** state — so a read-modify-write stays atomic within one
//! transaction, with no window for another commit to slip between the read and
//! the write. Both expose the *same* value/filesystem reads.
//!
//! The two differ only in a handful of **primitives** — how a path resolves, how a
//! blob is fetched (a reader amortizes both through generation-keyed caches; a
//! writer reads its own tables directly, never the caches), and how a principal is
//! authorized. Those primitives are the [`Grab`] trait; every actual read method
//! is a free function written once against `&dyn ReadOps`, and each transaction
//! type exposes them as thin inherent wrappers.

use super::IndexQuery;
use crate::{
    access::{ArchivedReader, Reader},
    codec::{decode, ArchivedDir, ArchivedValue},
    data::{AData, ARef, AValue, Scalar},
    engine::{entry_split, get_entry_kind},
    entry::{Entry, EntryKind},
    error::{AdbError, AdbResult},
    index::{registry::IndexEntry, Pattern},
    path::{APath, IntoArborPath, IntoValuePath, VPath},
    value::Value,
    AKey,
};

use std::{collections::HashSet, sync::Arc};

#[cfg(feature = "permissions")]
use crate::{
    acl::{AclClass, Rights},
    inode::Acl,
};

#[cfg(feature = "entry-timestamps")]
use crate::inode::NodeTimestamps;

/// The transaction-specific primitives the shared read surface is built on.
///
/// A reader implements these over a committed snapshot (amortized by its caches);
/// a writer implements them over its own uncommitted tables (never the caches).
/// The trait is object-safe on purpose, so [`IndexQuery`] can hold a `&dyn ReadOps`
/// and every read method can be one generic-free free function.
pub(crate) trait Grab {
    /// Resolves an access path to a vnode key. The table root always resolves (to
    /// [`AKey::ROOT`]) even before it is materialized; any other missing component
    /// is `None`.
    fn resolve(&self, path: &APath) -> AdbResult<Option<AKey>>;

    /// The entry blob for `akey`, or `None` if the vnode is absent.
    fn entry_blob(&self, akey: AKey) -> AdbResult<Option<Arc<Vec<u8>>>>;

    /// Looks up a registered index by name (empty registry ⇒ `None`).
    fn lookup_index(&self, index: &str) -> AdbResult<Option<IndexEntry>>;

    /// Scans `entry`'s index for the entities whose encoded columns start with
    /// `cols`, in ascending index order.
    fn scan_index(&self, entry: &IndexEntry, cols: &[u8]) -> AdbResult<Vec<AKey>>;

    /// The entities `pattern` matches at or under `root`, for subtree scoping.
    fn affected_under(&self, pattern: &Pattern, root: &APath) -> AdbResult<Vec<AKey>>;

    /// Checks the `needed` grade on `akey`. A no-op when enforcement is off.
    #[cfg(feature = "permissions")]
    fn authorize(&self, akey: AKey, needed: Rights) -> AdbResult<()>;

    /// Verifies `akey`'s integrity tag over `blob`. A no-op unless this handle holds
    /// the integrity key.
    #[cfg(feature = "permissions")]
    fn verify(&self, akey: AKey, blob: &[u8]) -> AdbResult<()>;

    /// Reads `akey`'s ACL, if it has one.
    #[cfg(feature = "permissions")]
    fn acl_of(&self, akey: AKey) -> AdbResult<Option<Acl>>;

    /// Resolves a group name to its id.
    #[cfg(feature = "permissions")]
    fn gid_of(&self, group: &str) -> AdbResult<Option<u32>>;

    /// The name of the user with id `uid`.
    #[cfg(feature = "permissions")]
    fn user_name(&self, uid: u32) -> AdbResult<Option<String>>;

    /// The name of the group with id `gid`.
    #[cfg(feature = "permissions")]
    fn group_name(&self, gid: u32) -> AdbResult<Option<String>>;

    /// Records a content read of `akey`, to be persisted with the access log.
    #[cfg(feature = "entry-timestamps")]
    fn record_access(&self, akey: AKey);

    /// The recorded timestamps of `akey`, or `None` if it has none.
    #[cfg(feature = "entry-timestamps")]
    fn timestamps_of(&self, akey: AKey) -> AdbResult<Option<NodeTimestamps>>;
}

/// Loads a typed value from the file at `path`, or `None` if absent. Errors if
/// `path` names a directory.
pub(crate) fn load<T: AData>(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Option<T>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    #[cfg(feature = "entry-timestamps")]
    src.record_access(akey);

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    match get_entry_kind(&blob)? {
        EntryKind::File => {
            // Skip the one-byte entry tag; the value payload starts at offset 1.
            let reader = ArchivedReader::new(blob, 1)?;

            Ok(Some(T::load(&reader, &VPath::root())?))
        }
        EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
    }
}

/// Loads a file stored at `path` into any [`serde::de::DeserializeOwned`] value,
/// reading the value blob directly over the zero-copy codec. `None` if absent;
/// errors if `path` names a directory.
#[cfg(feature = "serde")]
pub(crate) fn load_serde_value<T: serde::de::DeserializeOwned>(
    src: &dyn Grab,
    path: impl IntoArborPath,
) -> AdbResult<Option<T>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    #[cfg(feature = "entry-timestamps")]
    src.record_access(akey);

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    let (kind, payload) = entry_split(&blob)?;
    match kind {
        EntryKind::File => Ok(Some(crate::serde::from_blob(payload)?)),
        EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
    }
}

/// Loads the whole dynamic [`Value`] stored in the file at `path`, or `None` if
/// there is nothing there. Errors if `path` names a directory.
pub(crate) fn load_value(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Option<Value>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    #[cfg(feature = "entry-timestamps")]
    src.record_access(akey);

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    let (kind, payload) = entry_split(&blob)?;
    match kind {
        EntryKind::File => Ok(Some(decode(payload)?)),
        EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
    }
}

/// Opens a read accessor over the file at `path`, navigating its value blob
/// zero-copy. `None` if the file is absent; errors if `path` names a directory.
/// The accessor owns a snapshot of the blob, so it may outlive the transaction.
pub(crate) fn fetch<A: ARef<'static>>(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Option<A>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    #[cfg(feature = "entry-timestamps")]
    src.record_access(akey);

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    match get_entry_kind(&blob)? {
        EntryKind::File => {
            let reader: Arc<dyn Reader> = Arc::new(ArchivedReader::new(blob, 1)?);

            Ok(Some(A::open(reader, VPath::root())))
        }
        EntryKind::Dir => Err(AdbError::CannotAccess(format!("'{path}' is a directory, not a file"))),
    }
}

/// Reads the scalar at `at` inside the file at `path`, navigating the value blob
/// zero-copy. `None` if the file or the inner path is absent, or if the inner
/// path does not land on a scalar leaf.
pub(crate) fn get(src: &dyn Grab, path: impl IntoArborPath, at: impl IntoValuePath) -> AdbResult<Option<Scalar>> {
    let path = path.into_arbor_path()?;
    let at = at.into_value_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    #[cfg(feature = "entry-timestamps")]
    src.record_access(akey);

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    let (kind, payload) = entry_split(&blob)?;
    if kind != EntryKind::File {
        return Ok(None);
    }

    let Some(node) = ArchivedValue::new(payload)?.root().navigate(&at)? else {
        return Ok(None);
    };

    node.scalar_if_leaf()
}

/// Reads a typed scalar at `at` inside the file at `path`.
pub(crate) fn get_as<V: AValue>(
    src: &dyn Grab,
    path: impl IntoArborPath,
    at: impl IntoValuePath,
) -> AdbResult<Option<V>> {
    match get(src, path, at)? {
        Some(scalar) => Ok(Some(V::from_scalar_owned(scalar)?)),
        None => Ok(None),
    }
}

/// The filesystem kind (file or directory) at `path`, or `None` if absent.
///
/// Under the `permissions` feature this is a full read of the target vnode, not a
/// cheap stat: it requires `Rights::Access` on the vnode *itself* (not merely the
/// traversal right on its ancestors, which `resolve` already checks) and verifies
/// the vnode's own integrity tag. So it can neither probe the existence or kind of a
/// vnode the caller may not read, nor report a tampered kind tag — matching the
/// guarantees of `load`/`fetch`/`get`.
pub(crate) fn kind(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Option<EntryKind>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    Ok(Some(get_entry_kind(&blob)?))
}

/// Whether a file or directory exists at `path`.
///
/// Shares `kind`'s access rules: under `permissions` this reports `Err` rather than
/// `Ok(false)` for a vnode the caller lacks `Rights::Access` on, so existence cannot
/// be probed past an ACL that denies access (same behavior as a `load` on it).
pub(crate) fn exists(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<bool> {
    Ok(kind(src, path)?.is_some())
}

/// Lists the direct children of the directory at `path`, as `(name, kind)` pairs
/// in name order. Errors if `path` names a file.
///
/// Scope note (`permissions`): the directory itself is ACL-checked (`Rights::Access`)
/// and integrity-verified, but each child's reported `EntryKind` is read from that
/// child's own entry *without* re-authorizing or verifying the child — a listing
/// deliberately avoids paying a full ACL + MAC check per entry. A child kind reported
/// here therefore does not carry the tamper/authorization guarantee that a direct
/// `load`/`fetch`/`get`/`kind` on that child does: those run the per-vnode checks and
/// will catch a tampered child entry this listing does not.
pub(crate) fn ls(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Vec<Entry>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Err(AdbError::ValueNotFound(path));
    };

    #[cfg(feature = "permissions")]
    src.authorize(akey, Rights::Access)?;

    #[cfg(feature = "entry-timestamps")]
    src.record_access(akey);

    let Some(blob) = src.entry_blob(akey)? else {
        return Ok(Vec::new()); // the root directory, not yet materialized
    };

    #[cfg(feature = "permissions")]
    src.verify(akey, &blob)?;

    let (kind, payload) = entry_split(&blob)?;
    if kind != EntryKind::Dir {
        return Err(AdbError::CannotAccess(format!("'{path}' is a file, not a directory")));
    }

    let dir = ArchivedDir::new(payload)?;
    let mut out = Vec::with_capacity(dir.len()?);
    for (name, child) in dir.entries()? {
        let child_blob = src
            .entry_blob(child)?
            .ok_or_else(|| AdbError::Corrupt("a directory entry points at a missing vnode".into()))?;

        out.push(Entry::new(name.to_string(), get_entry_kind(&child_blob)?));
    }

    Ok(out)
}

/// The created / modified / accessed timestamps of the vnode at `path`, or `None`
/// if the vnode is absent or has no recorded metadata yet.
#[cfg(feature = "entry-timestamps")]
pub(crate) fn times(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Option<NodeTimestamps>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    src.timestamps_of(akey)
}

/// The [`Rights`] the file or directory at `path` grants `class`. Errors with
/// [`ValueNotFound`](AdbError::ValueNotFound) if nothing exists at `path`.
#[cfg(feature = "permissions")]
pub(crate) fn get_acl(src: &dyn Grab, path: impl IntoArborPath, class: AclClass) -> AdbResult<Rights> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Err(AdbError::ValueNotFound(path));
    };

    let Some(acl) = src.acl_of(akey)? else {
        return Ok(Rights::None);
    };

    Ok(match class {
        AclClass::User => acl.owner_rights(),
        AclClass::Other => acl.other_rights(),
        AclClass::Group(name) => match src.gid_of(&name)? {
            Some(gid) => acl.group_rights(gid),
            None => Rights::None,
        },
    })
}

/// The name of the owner of the file or directory at `path`, or `None` if the
/// vnode is absent or has no ACL. Falls back to the numeric id if the owning user
/// is no longer in the store.
#[cfg(feature = "permissions")]
pub(crate) fn owner(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Option<String>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(None);
    };

    let Some(acl) = src.acl_of(akey)? else {
        return Ok(None);
    };

    let name = src
        .user_name(acl.owner_uid())?
        .unwrap_or_else(|| acl.owner_uid().to_string());

    Ok(Some(name))
}

/// The names of the groups the file or directory at `path` belongs to, sorted;
/// empty when the vnode is absent, has no ACL, or is in no group.
#[cfg(feature = "permissions")]
pub(crate) fn groups(src: &dyn Grab, path: impl IntoArborPath) -> AdbResult<Vec<String>> {
    let path = path.into_arbor_path()?;

    let Some(akey) = src.resolve(&path)? else {
        return Ok(Vec::new());
    };

    let Some(acl) = src.acl_of(akey)? else {
        return Ok(Vec::new());
    };

    let mut names = Vec::new();
    for gid in acl.group_ids() {
        if let Some(name) = src.group_name(gid)? {
            names.push(name);
        }
    }
    names.sort();

    Ok(names)
}

/// Finds the entities an index points at, recomposing each as a `T`.
pub(crate) fn find<T: AData>(src: &dyn Grab, index: &str, values: &[Scalar]) -> AdbResult<Vec<T>> {
    IndexQuery::new(src, index).prefixed(values).run()
}

/// Runs a built index query: a prefix scan (exact = full prefix), optional subtree
/// scoping, optional reversal, then recomposes each hit as a `T`.
pub(crate) fn execute_query<T: AData>(
    src: &dyn Grab,
    index: &str,
    prefix: &[Scalar],
    reverse: bool,
    root: &APath,
) -> AdbResult<Vec<T>> {
    let entry = src.lookup_index(index)?.ok_or_else(|| AdbError::IndexNotFound {
        index: index.to_string(),
    })?;

    let def = entry.def();
    if prefix.len() > def.columns().len() {
        return Err(AdbError::IndexArity {
            index:    index.to_string(),
            expected: def.columns().len(),
            got:      prefix.len(),
        });
    }

    // `encode_columns` zips with the columns, so a short `prefix` encodes only its
    // leading columns — exactly the byte prefix a prefix scan needs.
    let cols = def.encode_columns(prefix);
    let mut entities = src.scan_index(&entry, &cols)?;

    // Restrict to entities at or under `root` (a rooted view scopes here).
    if !root.is_empty() {
        let pattern = Pattern::parse(def.pattern())?;
        if pattern.depth() < root.len() {
            return Ok(Vec::new());
        }

        let under: HashSet<AKey> = src.affected_under(&pattern, root)?.into_iter().collect();
        entities.retain(|entity| under.contains(entity));
    }

    // The scan yields ascending index order; reverse the materialized hits for
    // descending order.
    if reverse {
        entities.reverse();
    }

    // Each match is addressed by its stable key; recompose it from its own blob.
    let mut out = Vec::with_capacity(entities.len());
    for entity in entities {
        if let Some(value) = load_entity::<T>(src, entity)? {
            out.push(value);
        }
    }

    Ok(out)
}

/// Recomposes the file stored under `entity` as a `T`, or `None` when the entity
/// is absent or is a directory (not a decodable value).
fn load_entity<T: AData>(src: &dyn Grab, entity: AKey) -> AdbResult<Option<T>> {
    // An index query silently skips entities the principal may not read.
    #[cfg(feature = "permissions")]
    match src.authorize(entity, Rights::Access) {
        Ok(()) => {}
        Err(AdbError::PermissionDenied(_)) => return Ok(None),
        Err(err) => return Err(err),
    }

    let Some(blob) = src.entry_blob(entity)? else {
        return Ok(None);
    };

    #[cfg(feature = "permissions")]
    src.verify(entity, &blob)?;

    match get_entry_kind(&blob)? {
        EntryKind::File => {
            let reader = ArchivedReader::new(blob, 1)?;

            Ok(Some(T::load(&reader, &VPath::root())?))
        }
        EntryKind::Dir => Ok(None),
    }
}
