//! The cryptographic primitives behind the `permissions` feature.
//!
//! Passwords are never stored. A per-user Argon2id-derived key-encryption key
//! (KEK) wraps the database secrets with an AEAD; authenticating *is* unwrapping
//! them, so a wrong password fails the AEAD tag rather than matching a stored hash.
//!
//! Two secrets are wrapped together (`K ‖ sk`): the integrity key `K`, which keys
//! the per-value BLAKE3 MAC an authenticated reader verifies, and the Ed25519
//! signing seed `sk`, whose signature a keyless guest verifies with the public key.
//! Only an authenticated user unwraps them, so only an authenticated user can seal
//! (write) a value; the guest holds neither and is read-only.

use crate::error::{AdbError, AdbResult};
use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305,
    XNonce,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

/// Length of the database integrity key `K`.
pub(crate) const KEY_LEN: usize = 32;

/// Length of an Ed25519 signing seed (the private "write" key).
pub(crate) const SEED_LEN: usize = 32;

/// Length of an Ed25519 public verification key (the public "read" key).
pub(crate) const PUBKEY_LEN: usize = 32;

/// Length of an Ed25519 signature.
pub(crate) const SIG_LEN: usize = 64;

/// Length of a per-user Argon2 salt.
pub(crate) const SALT_LEN: usize = 16;

/// Length of an XChaCha20-Poly1305 nonce.
pub(crate) const NONCE_LEN: usize = 24;

/// Fills `buf` with cryptographically secure random bytes.
pub(crate) fn random_bytes(buf: &mut [u8]) -> AdbResult<()> {
    getrandom::fill(buf).map_err(|e| AdbError::CannotAccess(format!("secure RNG failed: {e}")))
}

/// A fresh random integrity key.
pub(crate) fn random_key() -> AdbResult<[u8; KEY_LEN]> {
    let mut key = [0u8; KEY_LEN];
    random_bytes(&mut key)?;

    Ok(key)
}

/// A fresh random Ed25519 signing seed (any 32 bytes is a valid seed).
pub(crate) fn random_seed() -> AdbResult<[u8; SEED_LEN]> {
    let mut seed = [0u8; SEED_LEN];
    random_bytes(&mut seed)?;

    Ok(seed)
}

/// The public verification key matching the signing `seed`.
pub(crate) fn public_key(seed: &[u8; SEED_LEN]) -> [u8; PUBKEY_LEN] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

/// Signs `msg` with the private signing `seed`, returning the detached signature.
pub(crate) fn sign(seed: &[u8; SEED_LEN], msg: &[u8]) -> [u8; SIG_LEN] {
    SigningKey::from_bytes(seed).sign(msg).to_bytes()
}

/// Whether `sig` is a valid signature of `msg` under `pubkey`. A malformed public
/// key or signature simply fails to verify (`false`), never panics. Uses strict
/// verification, rejecting the non-canonical encodings Ed25519 would otherwise admit.
pub(crate) fn verify(pubkey: &[u8; PUBKEY_LEN], msg: &[u8], sig: &[u8; SIG_LEN]) -> bool {
    let Ok(verifying) = VerifyingKey::from_bytes(pubkey) else {
        return false;
    };

    verifying.verify_strict(msg, &Signature::from_bytes(sig)).is_ok()
}

/// Derives a 32-byte key-encryption key from `password` and `salt` (Argon2id).
fn derive_kek(password: &str, salt: &[u8]) -> AdbResult<[u8; 32]> {
    let mut kek = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut kek)
        .map_err(|e| AdbError::CannotAccess(format!("key derivation failed: {e}")))?;

    Ok(kek)
}

/// Wraps `key` under `password`, returning `(salt, nonce, ciphertext)`.
pub(crate) fn wrap_key(password: &str, key: &[u8]) -> AdbResult<([u8; SALT_LEN], [u8; NONCE_LEN], Vec<u8>)> {
    let mut salt = [0u8; SALT_LEN];
    random_bytes(&mut salt)?;

    let mut nonce = [0u8; NONCE_LEN];
    random_bytes(&mut nonce)?;

    let kek = derive_kek(password, &salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&kek)
        .map_err(|e| AdbError::CannotAccess(format!("cipher init failed: {e}")))?;

    let xnonce = XNonce::try_from(&nonce[..]).map_err(|_| {
        const MSG: &str = "nonce has the wrong length";

        AdbError::CannotAccess(MSG.into())
    })?;

    let ciphertext = cipher
        .encrypt(&xnonce, key)
        .map_err(|_| AdbError::CannotAccess(String::from("wrapping the integrity key failed")))?;

    Ok((salt, nonce, ciphertext))
}

/// Unwraps a key produced by [`wrap_key`], or [`AuthenticationFailed`] when
/// `password` is wrong (the AEAD tag will not verify).
pub(crate) fn unwrap_key(password: &str, salt: &[u8], nonce: &[u8], ciphertext: &[u8]) -> AdbResult<Vec<u8>> {
    let kek = derive_kek(password, salt)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&kek)
        .map_err(|e| AdbError::CannotAccess(format!("cipher init failed: {e}")))?;

    let xnonce = XNonce::try_from(nonce).map_err(|_| AdbError::AuthenticationFailed)?;

    cipher
        .decrypt(&xnonce, ciphertext)
        .map_err(|_| AdbError::AuthenticationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_round_trips_and_rejects_a_wrong_password() {
        let key = random_key().unwrap();
        let (salt, nonce, wrapped) = wrap_key("correct horse", &key).unwrap();

        assert_eq!(
            unwrap_key("correct horse", &salt, &nonce, &wrapped).unwrap(),
            key.to_vec()
        );
        assert!(matches!(
            unwrap_key("battery staple", &salt, &nonce, &wrapped),
            Err(AdbError::AuthenticationFailed)
        ));
    }

    #[test]
    fn signatures_verify_under_the_matching_public_key_only() {
        let seed = random_seed().unwrap();
        let pubkey = public_key(&seed);
        let sig = sign(&seed, b"a signed message");

        // The genuine signature verifies; a different message or key does not.
        assert!(verify(&pubkey, b"a signed message", &sig));
        assert!(!verify(&pubkey, b"a tampered message", &sig));
        assert!(!verify(&public_key(&random_seed().unwrap()), b"a signed message", &sig));

        // A flipped signature byte is rejected rather than panicking.
        let mut broken = sig;
        broken[0] ^= 0x01;
        assert!(!verify(&pubkey, b"a signed message", &broken));
    }
}
