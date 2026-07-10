//! Integrity tags over the control plane and each vnode's value.
//!
//! Two tamper checks cover a value, over exactly the same bytes — the vnode's
//! stored entry *and* its ACL, so tampering with either the data or its
//! permissions is caught:
//! - a **keyed BLAKE3 MAC**, keyed by the integrity key `K` (unlocked at authentication), that an authenticated reader
//!   verifies — fast, and unforgeable without `K`;
//! - an **Ed25519 signature** that a keyless *guest* verifies with the public key. The guest can verify but not forge
//!   (it lacks the private signing seed), which is exactly the read/write split: only an authenticated user can seal a
//!   value.
//!
//! The **control** tag is a keyed MAC binding a monotonic `epoch` to the user and
//! group blobs *and the public verification key*, so none can be rolled back,
//! spliced, or (the public key) swapped on its own — a swap of the public key,
//! which would otherwise fool the guest, is caught at authentication.
//!
//! A single-file store cannot, on its own, detect a wholesale rollback to an
//! earlier complete-and-consistent snapshot (there is no anchor outside the file);
//! the epoch defeats partial/spliced rollback of the control plane, which is the
//! reachable attack for a redb-only editor. A guest, holding no secret, likewise
//! cannot detect an attacker that swaps the public key *and* re-signs a value — its
//! verification covers accidental corruption and naive tampering.

use crate::{crypto, AKey};
use blake3::Hasher;

/// The length of an integrity tag.
pub(crate) const MAC_LEN: usize = 32;

/// Domain-separation tag for a per-vnode value tag (MAC and signature alike).
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

/// Absorbs the domain-separated, length-framed bytes a value tag binds: the table
/// name, vnode key, entry blob, and encoded ACL. Shared by the MAC and the
/// signature so both cover byte-for-byte the same message.
fn absorb_value(hasher: &mut Hasher, table: &str, akey: AKey, blob: &[u8], acl: &[u8]) {
    hasher.update(&[DOMAIN_VALUE]);
    field(hasher, table.as_bytes());
    hasher.update(&akey.into_bytes());
    field(hasher, blob);
    field(hasher, acl);
}

/// The keyed MAC of vnode `akey`'s stored entry `blob` and its encoded `acl` in
/// `table` — verified by an authenticated reader (fast, and unforgeable without `K`).
pub(crate) fn mac_value(key: &[u8], table: &str, akey: AKey, blob: &[u8], acl: &[u8]) -> [u8; MAC_LEN] {
    let mut hasher = keyed(key);
    absorb_value(&mut hasher, table, akey, blob, acl);

    *hasher.finalize().as_bytes()
}

/// The unkeyed digest of the same message [`mac_value`] covers. The value signature
/// signs this digest (hash-then-sign), so a large blob is streamed through BLAKE3
/// rather than gathered into one contiguous buffer to sign.
fn value_digest(table: &str, akey: AKey, blob: &[u8], acl: &[u8]) -> [u8; MAC_LEN] {
    let mut hasher = Hasher::new();
    absorb_value(&mut hasher, table, akey, blob, acl);

    *hasher.finalize().as_bytes()
}

/// Signs vnode `akey`'s entry `blob` and encoded `acl` with `signer`, producing the
/// tag a keyless guest verifies with [`verify_value`]. `signer` holds the expanded
/// signing key, so signing a value never re-derives it from the seed.
pub(crate) fn sign_value(
    signer: &crypto::Signer,
    table: &str,
    akey: AKey,
    blob: &[u8],
    acl: &[u8],
) -> [u8; crypto::SIG_LEN] {
    signer.sign(&value_digest(table, akey, blob, acl))
}

/// Whether `sig` is a valid signature of vnode `akey`'s entry `blob` and encoded
/// `acl` under `pubkey` — the check a keyless guest runs on every read.
pub(crate) fn verify_value(
    pubkey: &[u8; crypto::PUBKEY_LEN],
    table: &str,
    akey: AKey,
    blob: &[u8],
    acl: &[u8],
    sig: &[u8; crypto::SIG_LEN],
) -> bool {
    crypto::verify(pubkey, &value_digest(table, akey, blob, acl), sig)
}

/// The MAC of the control plane: `epoch` bound to the raw user and group blobs and
/// the public verification key.
pub(crate) fn mac_control(key: &[u8], epoch: u64, users: &[u8], groups: &[u8], pubkey: &[u8]) -> [u8; MAC_LEN] {
    let mut hasher = keyed(key);
    hasher.update(&[DOMAIN_CONTROL]);
    hasher.update(&epoch.to_be_bytes());
    field(&mut hasher, users);
    field(&mut hasher, groups);
    field(&mut hasher, pubkey);

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
    fn control_mac_binds_the_epoch_and_public_key() {
        let key = [3u8; MAC_LEN];
        let pk = [9u8; crypto::PUBKEY_LEN];
        let a = mac_control(&key, 1, b"users", b"groups", &pk);
        let b = mac_control(&key, 2, b"users", b"groups", &pk);

        assert_ne!(a, b);
        assert_eq!(a, mac_control(&key, 1, b"users", b"groups", &pk));

        // Swapping the public key changes the tag, so a swap is caught at auth.
        assert_ne!(
            a,
            mac_control(&key, 1, b"users", b"groups", &[10u8; crypto::PUBKEY_LEN])
        );
    }

    #[test]
    fn value_signature_verifies_and_binds_every_field() {
        let seed = crypto::random_seed().unwrap();
        let pubkey = crypto::public_key(&seed);
        let akey = AKey::from(1u128);
        let signer = crypto::Signer::new(&seed);
        let sig = sign_value(&signer, "t", akey, b"blob", b"acl");

        // The genuine signature verifies; a different table, key, blob, or ACL fails.
        assert!(verify_value(&pubkey, "t", akey, b"blob", b"acl", &sig));
        assert!(!verify_value(&pubkey, "u", akey, b"blob", b"acl", &sig));
        assert!(!verify_value(&pubkey, "t", AKey::from(2u128), b"blob", b"acl", &sig));
        assert!(!verify_value(&pubkey, "t", akey, b"bloc", b"acl", &sig));
        assert!(!verify_value(&pubkey, "t", akey, b"blob", b"acm", &sig));

        // A signature from a different seed does not verify under this public key.
        let other_signer = crypto::Signer::new(&crypto::random_seed().unwrap());
        let other = sign_value(&other_signer, "t", akey, b"blob", b"acl");
        assert!(!verify_value(&pubkey, "t", akey, b"blob", b"acl", &other));
    }

    #[test]
    fn ct_eq_matches_only_identical_slices() {
        assert!(ct_eq(b"abcd", b"abcd"));
        assert!(!ct_eq(b"abcd", b"abce"));
        assert!(!ct_eq(b"abcd", b"abc"));
    }
}
