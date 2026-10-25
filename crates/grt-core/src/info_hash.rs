use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// A BitTorrent v1 (BEP 3) infohash: the SHA-1 of a torrent's info dictionary.
///
/// Packs are transferred as v1 torrents, so each entry in a manifest's pack
/// chain is one of these. See ADR-0006.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InfoHash([u8; 20]);

/// A BitTorrent v2 (BEP 52) infohash: the SHA-256 of a torrent's info dictionary.
///
/// Reserved for the manifest's optional `packs2` field, which lets a future
/// v2-capable transfer engine find the same packs. See ADR-0006.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InfoHashV2([u8; 32]);

impl InfoHash {
    /// Length of a v1 infohash in bytes.
    pub const LEN: usize = 20;

    /// Returns the raw infohash bytes.
    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl InfoHashV2 {
    /// Length of a v2 infohash in bytes.
    pub const LEN: usize = 32;

    /// Returns the raw infohash bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 20]> for InfoHash {
    fn from(bytes: [u8; 20]) -> Self {
        InfoHash(bytes)
    }
}

impl From<[u8; 32]> for InfoHashV2 {
    fn from(bytes: [u8; 32]) -> Self {
        InfoHashV2(bytes)
    }
}

impl TryFrom<&[u8]> for InfoHash {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        <[u8; 20]>::try_from(bytes)
            .map(InfoHash)
            .map_err(|_| Error::InvalidLength {
                what: "v1 infohash",
                expected: "20",
                actual: bytes.len(),
            })
    }
}

impl TryFrom<&[u8]> for InfoHashV2 {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        <[u8; 32]>::try_from(bytes)
            .map(InfoHashV2)
            .map_err(|_| Error::InvalidLength {
                what: "v2 infohash",
                expected: "32",
                actual: bytes.len(),
            })
    }
}

impl FromStr for InfoHash {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidHex {
            what: "v1 infohash",
        })?;
        InfoHash::try_from(bytes.as_slice())
    }
}

impl FromStr for InfoHashV2 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidHex {
            what: "v2 infohash",
        })?;
        InfoHashV2::try_from(bytes.as_slice())
    }
}

impl fmt::Display for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Display for InfoHashV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let v1 = "0f".repeat(20);
        assert_eq!(
            v1.parse::<InfoHash>().unwrap().to_string(),
            v1,
            "v1 Display matches the parsed hex"
        );
        let v2 = "0f".repeat(32);
        assert_eq!(
            v2.parse::<InfoHashV2>().unwrap().to_string(),
            v2,
            "v2 Display matches the parsed hex"
        );
    }

    #[test]
    fn versions_do_not_mix() {
        assert!(
            "0f".repeat(32).parse::<InfoHash>().is_err(),
            "a v2 hash is not a v1 hash"
        );
        assert!(
            "0f".repeat(20).parse::<InfoHashV2>().is_err(),
            "a v1 hash is not a v2 hash"
        );
    }
}
