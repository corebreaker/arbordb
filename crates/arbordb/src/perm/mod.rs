//! The `permissions` feature: user/password authentication and per-value ACLs
//! over a *protected* database.
//!
//! A protected database records `permissions` in its `required_features`, so a
//! binary built without this feature refuses to open it (see
//! [`AdbError::DatabaseProtected`](crate::AdbError::DatabaseProtected)). Users,
//! groups, and the key-wrapping keyring live as blobs in the reserved `$metadata`
//! table; per-value ACLs live in the `$inodes` table alongside the timestamps.
//!
//! A database is protected only via [`ArborDb::change_password`](crate::ArborDb::change_password)
//! on a non-protected handle, which promotes it and authenticates the caller as
//! the master user. Creation itself never produces a protected database.

mod principal;
mod pubkey;

pub(crate) mod access;
pub(crate) mod integrity;
pub(crate) mod store;

pub mod constants;

pub(crate) use principal::Principal;

pub use pubkey::PublicKey;
