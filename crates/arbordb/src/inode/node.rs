use super::{constants::SECTION_TIMESTAMPS, underlying_timestamps::UnderlyingTimestamps};
use crate::{
    codec::{put_bytes, Reader},
    error::AdbResult,
};

#[cfg(feature = "permissions")]
use super::{
    acl::Acl,
    constants::{SECTION_ACL, SECTION_MAC},
};

/// A decoded inode: the sections this build understands, plus any it does not —
/// kept verbatim so a rewrite never drops another feature's metadata.
#[derive(Default)]
pub(super) struct Inode {
    timestamps: Option<UnderlyingTimestamps>,

    #[cfg(feature = "permissions")]
    acl: Option<Acl>,

    #[cfg(feature = "permissions")]
    mac: Option<[u8; 32]>,

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
                other => inode.extra.push((other, body.to_vec())),
            }
        }

        Ok(inode)
    }

    pub(super) fn encode(&self) -> Vec<u8> {
        let mut sections: Vec<(u8, Vec<u8>)> = Vec::new();
        if let Some(times) = self.timestamps {
            sections.push((SECTION_TIMESTAMPS, times.encode()));
        }
        #[cfg(feature = "permissions")]
        if let Some(acl) = self.acl {
            sections.push((SECTION_ACL, acl.encode()));
        }
        #[cfg(feature = "permissions")]
        if let Some(mac) = self.mac {
            sections.push((SECTION_MAC, mac.to_vec()));
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

    pub(super) fn timestamps(&self) -> Option<UnderlyingTimestamps> {
        self.timestamps
    }

    pub(super) fn timestamps_mut(&mut self) -> Option<&mut UnderlyingTimestamps> {
        self.timestamps.as_mut()
    }

    pub(super) fn set_timestamps(&mut self, timestamps: UnderlyingTimestamps) {
        self.timestamps.replace(timestamps);
    }

    #[cfg(feature = "permissions")]
    pub(super) fn acl(&self) -> Option<Acl> {
        self.acl
    }

    #[cfg(feature = "permissions")]
    pub(super) fn set_acl(&mut self, acl: Acl) {
        self.acl.replace(acl);
    }

    #[cfg(feature = "permissions")]
    pub(super) fn mac(&self) -> Option<[u8; 32]> {
        self.mac
    }

    #[cfg(feature = "permissions")]
    pub(super) fn set_mac(&mut self, mac: [u8; 32]) {
        self.mac.replace(mac);
    }
}
