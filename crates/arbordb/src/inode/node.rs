//! [`Inode`] — the decoded in-memory form of an `$inodes` blob: the sections this
//! build understands (timestamps, and with `permissions` an ACL and integrity tag)
//! plus any unrecognized sections kept verbatim in `extra`. The section-tagged,
//! copy-through layout is what lets a build read and rewrite an inode written by a
//! build with a different feature set.

use super::{constants::SECTION_TIMESTAMPS, underlying_timestamps::UnderlyingTimestamps};
use crate::{
    codec::{put_bytes, Reader},
    error::AdbResult,
};

#[cfg(feature = "permissions")]
use super::{
    acl::Acl,
    constants::{SECTION_ACL, SECTION_MAC, SECTION_SIG},
};

#[cfg(feature = "permissions")]
use crate::crypto::SIG_LEN;

/// A decoded inode: the sections this build understands, plus any it does not —
/// kept verbatim so a rewrite never drops another feature's metadata.
#[derive(Default)]
pub(super) struct Inode {
    /// The created/modified/accessed section, if present.
    timestamps: Option<UnderlyingTimestamps>,

    /// The access-control section (`permissions` feature), if present.
    #[cfg(feature = "permissions")]
    acl: Option<Acl>,

    /// The 32-byte keyed integrity tag (`permissions` feature), if present.
    #[cfg(feature = "permissions")]
    mac: Option<[u8; 32]>,

    /// The 64-byte Ed25519 value signature (`permissions` feature), if present.
    #[cfg(feature = "permissions")]
    sig: Option<[u8; SIG_LEN]>,

    /// Sections this build does not recognize, kept as `(tag, body)` and written
    /// back untouched so another feature's metadata survives a rewrite.
    extra: Vec<(u8, Vec<u8>)>,
}

impl Inode {
    /// Decodes a `[count:u8]` header then that many `[tag:u8][len:u32][body]`
    /// sections; unrecognized tags are retained in `extra`.
    pub(super) fn decode(bytes: &[u8]) -> AdbResult<Self> {
        let mut r = Reader::new(bytes);
        let count = r.u8()?;

        let mut inode = Inode::default();
        for _ in 0..count {
            let tag = r.u8()?;
            let body = r.bytes()?;

            match tag {
                SECTION_TIMESTAMPS => inode.timestamps = Some(UnderlyingTimestamps::decode(body)?),
                #[cfg(feature = "permissions")]
                SECTION_ACL => inode.acl = Some(Acl::decode(body)?),
                #[cfg(feature = "permissions")]
                SECTION_MAC => {
                    inode.mac =
                        Some(body.try_into().map_err(|_| {
                            crate::error::AdbError::Corrupt("inode MAC section has a bad length".into())
                        })?);
                }
                #[cfg(feature = "permissions")]
                SECTION_SIG => {
                    inode.sig = Some(body.try_into().map_err(|_| {
                        crate::error::AdbError::Corrupt("inode signature section has a bad length".into())
                    })?);
                }
                other => inode.extra.push((other, body.to_vec())),
            }
        }

        Ok(inode)
    }

    /// Serializes the known sections followed by any retained `extra` ones, back
    /// into the `[count:u8]` + `[tag:u8][len:u32][body]` layout that [`decode`](Self::decode) reads.
    pub(super) fn encode(&self) -> Vec<u8> {
        let mut sections: Vec<(u8, Vec<u8>)> = Vec::new();
        if let Some(times) = self.timestamps {
            sections.push((SECTION_TIMESTAMPS, times.encode()));
        }

        #[cfg(feature = "permissions")]
        if let Some(acl) = &self.acl {
            sections.push((SECTION_ACL, acl.encode()));
        }

        #[cfg(feature = "permissions")]
        if let Some(mac) = self.mac {
            sections.push((SECTION_MAC, mac.to_vec()));
        }

        #[cfg(feature = "permissions")]
        if let Some(sig) = self.sig {
            sections.push((SECTION_SIG, sig.to_vec()));
        }

        for (tag, body) in &self.extra {
            sections.push((*tag, body.clone()));
        }

        let mut out = Vec::new();
        out.push(sections.len() as u8);
        for (tag, body) in &sections {
            out.push(*tag);
            put_bytes(&mut out, body);
        }

        out
    }

    /// The timestamps section, if any.
    pub(super) fn timestamps(&self) -> Option<UnderlyingTimestamps> {
        self.timestamps
    }

    /// A mutable borrow of the timestamps section, if any.
    pub(super) fn timestamps_mut(&mut self) -> Option<&mut UnderlyingTimestamps> {
        self.timestamps.as_mut()
    }

    /// Sets (or replaces) the timestamps section.
    pub(super) fn set_timestamps(&mut self, timestamps: UnderlyingTimestamps) {
        self.timestamps.replace(timestamps);
    }

    /// The ACL section, if any.
    #[cfg(feature = "permissions")]
    pub(super) fn acl(&self) -> Option<Acl> {
        self.acl.clone()
    }

    /// Sets (or replaces) the ACL section.
    #[cfg(feature = "permissions")]
    pub(super) fn set_acl(&mut self, acl: Acl) {
        self.acl.replace(acl);
    }

    /// The integrity-tag section, if any.
    #[cfg(feature = "permissions")]
    pub(super) fn mac(&self) -> Option<[u8; 32]> {
        self.mac
    }

    /// Sets (or replaces) the integrity-tag section.
    #[cfg(feature = "permissions")]
    pub(super) fn set_mac(&mut self, mac: [u8; 32]) {
        self.mac.replace(mac);
    }

    /// The value-signature section, if any.
    #[cfg(feature = "permissions")]
    pub(super) fn sig(&self) -> Option<[u8; SIG_LEN]> {
        self.sig
    }

    /// Sets (or replaces) the value-signature section.
    #[cfg(feature = "permissions")]
    pub(super) fn set_sig(&mut self, sig: [u8; SIG_LEN]) {
        self.sig.replace(sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::put_bytes;

    /// Builds an inode blob of `[count:u8]` then `[tag:u8][len-prefixed body]` sections.
    fn blob(sections: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = vec![sections.len() as u8];
        for (tag, body) in sections {
            out.push(*tag);
            put_bytes(&mut out, body);
        }

        out
    }

    #[test]
    fn an_unknown_section_is_kept_verbatim_across_a_round_trip() {
        // A section tag this build does not know is copied through untouched, so
        // another feature's metadata survives a decode/encode cycle.
        let original = blob(&[(200, &[1, 2, 3])]);
        let inode = Inode::decode(&original).unwrap();

        assert_eq!(inode.encode(), original);
    }

    #[cfg(feature = "permissions")]
    #[test]
    fn a_bad_length_mac_or_signature_section_is_rejected() {
        use super::super::constants::{SECTION_MAC, SECTION_SIG};

        // A MAC is 32 bytes and a signature 64; a shorter body is corruption.
        assert!(Inode::decode(&blob(&[(SECTION_MAC, &[1, 2, 3])])).is_err());
        assert!(Inode::decode(&blob(&[(SECTION_SIG, &[1, 2, 3])])).is_err());
    }
}
