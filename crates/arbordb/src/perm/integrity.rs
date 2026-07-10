//! Keyed BLAKE3 integrity tags over the control plane and each vnode's value.
//!
//! Every tag is keyed by the database integrity key `K`, unlocked at
//! authentication, so a party without `K` — for example a program that opens the
//! redb file directly to edit a table — cannot forge one. A **value** tag binds
//! the vnode's stored bytes *and* its ACL, so tampering with either the data or
//! its permissions is caught on the next authenticated read. The **control** tag
//! binds a monotonic `epoch` to the user and group blobs together, so neither can
//! be rolled back or spliced on its own.
//!
//! A single-file store cannot, on its own, detect a wholesale rollback to an
//! earlier complete-and-consistent snapshot (there is no anchor outside the file);
//! the epoch defeats partial/spliced rollback of the control plane, which is the
//! reachable attack for a redb-only editor.

use crate::AKey;

use blake3::Hasher;

/// The length of an integrity tag.
pub(crate) const MAC_LEN: usize = 32;

/// Domain-separation tag for a per-vnode value MAC.
const DOMAIN_VALUE: u8 = 1;

/// Domain-separation tag for the control-plane MAC.
const DOMAIN_CONTROL: u8 = 2;

/// A hasher keyed by the 32-byte integrity key. `K` is invariantly [`MAC_LEN`]
/// bytes throughout the crate, so the copy never truncates.
fn keyed(key: &[u8]) -> Hasher {
    let mut k = [0u8; MAC_LEN];
    k.copy_from_slice(key);

    Hasher::new_keyed(&k)
}

/// Absorbs a length-delimited field, so a concatenation of fields is unambiguous.
fn field(hasher: &mut Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

/// The MAC of vnode `akey`'s stored entry `blob` and its encoded `acl` in `table`.
pub(crate) fn mac_value(key: &[u8], table: &str, akey: AKey, blob: &[u8], acl: &[u8]) -> [u8; MAC_LEN] {
    let mut hasher = keyed(key);
    hasher.update(&[DOMAIN_VALUE]);
    field(&mut hasher, table.as_bytes());
    hasher.update(&akey.into_bytes());
    field(&mut hasher, blob);
    field(&mut hasher, acl);

    *hasher.finalize().as_bytes()
}

/// The MAC of the control plane: `epoch` bound to the raw user and group blobs.
pub(crate) fn mac_control(key: &[u8], epoch: u64, users: &[u8], groups: &[u8]) -> [u8; MAC_LEN] {
    let mut hasher = keyed(key);
    hasher.update(&[DOMAIN_CONTROL]);
    hasher.update(&epoch.to_be_bytes());
    field(&mut hasher, users);
    field(&mut hasher, groups);

    *hasher.finalize().as_bytes()
}

/// Constant-time equality of two byte slices (MAC comparison, so timing leaks no
/// information about how many leading bytes matched).
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }

    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_mac_changes_with_every_bound_input() {
        let key = [7u8; MAC_LEN];
        let akey = AKey::from(1u128);
        let base = mac_value(&key, "t", akey, b"blob", b"acl");

        // Same inputs → same tag.
        assert_eq!(base, mac_value(&key, "t", akey, b"blob", b"acl"));

        // A different key, table, key-id, blob, or ACL each changes the tag.
        assert_ne!(base, mac_value(&[8u8; MAC_LEN], "t", akey, b"blob", b"acl"));
        assert_ne!(base, mac_value(&key, "u", akey, b"blob", b"acl"));
        assert_ne!(base, mac_value(&key, "t", AKey::from(2u128), b"blob", b"acl"));
        assert_ne!(base, mac_value(&key, "t", akey, b"bloc", b"acl"));
        assert_ne!(base, mac_value(&key, "t", akey, b"blob", b"acm"));
    }

    #[test]
    fn field_framing_is_unambiguous() {
        // Without length framing these two field splits would collide; with it they
        // must not.
        let key = [1u8; MAC_LEN];
        let akey = AKey::ROOT;
        assert_ne!(
            mac_value(&key, "t", akey, b"ab", b"c"),
            mac_value(&key, "t", akey, b"a", b"bc"),
        );
    }

    #[test]
    fn control_mac_binds_the_epoch() {
        let key = [3u8; MAC_LEN];
        let a = mac_control(&key, 1, b"users", b"groups");
        let b = mac_control(&key, 2, b"users", b"groups");

        assert_ne!(a, b);
        assert_eq!(a, mac_control(&key, 1, b"users", b"groups"));
    }

    #[test]
    fn ct_eq_matches_only_identical_slices() {
        assert!(ct_eq(b"abcd", b"abcd"));
        assert!(!ct_eq(b"abcd", b"abce"));
        assert!(!ct_eq(b"abcd", b"abc"));
    }
}
