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
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305,
    XNonce,
};

// The `Signer` trait is brought in anonymously (`as _`) so its `sign` method
// resolves, without its name colliding with this module's own `Signer` struct.
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};

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

/// A reusable Ed25519 signer that holds the **expanded** signing key.
///
/// Building the key from a seed derives the public key (an elliptic-curve scalar
/// multiplication) — the costly half of signing. A session signs a value on every
/// protected write, so it expands the key once and reuses it, rather than
/// reconstructing it from the seed on each signature.
pub(crate) struct Signer {
    /// The expanded signing key, derived once from the seed.
    key: SigningKey,
}

impl Signer {
    /// Expands the signing key from `seed`, so later signatures reuse it.
    pub(crate) fn new(seed: &[u8; SEED_LEN]) -> Self {
        Self {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// Signs `msg`, returning the detached signature. Reuses the expanded key.
    pub(crate) fn sign(&self, msg: &[u8]) -> [u8; SIG_LEN] {
        self.key.sign(msg).to_bytes()
    }
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

/// Argon2id memory cost, in kibibytes (19 MiB).
const KDF_M_COST: u32 = 19 * 1024;

/// Argon2id time cost (number of passes).
const KDF_T_COST: u32 = 2;

/// Argon2id parallelism (number of lanes).
const KDF_P_COST: u32 = 1;

/// Derives a 32-byte key-encryption key from `password` and `salt` (Argon2id).
///
/// The cost parameters (`KDF_M_COST` / `KDF_T_COST` / `KDF_P_COST`) are pinned
/// explicitly rather than taken from `Argon2::default()`: this KDF is the sole
/// barrier between a stolen database file and the wrapped integrity key and signing
/// seed, so its security margin must not depend on a library default that a future
/// dependency bump could silently change. The chosen values match the current
/// OWASP-recommended Argon2id baseline (m = 19 MiB, t = 2, p = 1) and reproduce the
/// key `Argon2::default()` derived, so existing databases stay readable.
fn derive_kek(password: &str, salt: &[u8]) -> AdbResult<[u8; 32]> {
    let params = Params::new(KDF_M_COST, KDF_T_COST, KDF_P_COST, None)
        .map_err(|e| AdbError::CannotAccess(format!("invalid KDF parameters: {e}")))?;

    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut kek = [0u8; 32];
    argon
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
        let sig = Signer::new(&seed).sign(b"a signed message");

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
