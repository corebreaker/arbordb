//! The cryptographic primitives behind the `permissions` feature.
//!
//! Passwords are never stored. A per-user Argon2id-derived key-encryption key
//! (KEK) wraps the database integrity key `K` with an AEAD; authenticating *is*
//! unwrapping `K`, so a wrong password fails the AEAD tag rather than matching a
//! stored hash. `K` also keys the per-value BLAKE3 MAC used for tamper detection.

use crate::error::{AdbError, AdbResult};
use argon2::Argon2;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305,
    XNonce,
};

/// Length of the database integrity key `K`.
pub(crate) const KEY_LEN: usize = 32;

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
