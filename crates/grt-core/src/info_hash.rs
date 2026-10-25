use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// A BitTorrent v2 (BEP 52) infohash: the SHA-256 of a torrent's info dictionary.
///
/// Each packfile in a manifest's pack chain is identified by one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InfoHash([u8; 32]);

impl InfoHash {
    /// Length of an infohash in bytes.
    pub const LEN: usize = 32;

    /// Returns the raw infohash bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for InfoHash {
    fn from(bytes: [u8; 32]) -> Self {
        InfoHash(bytes)
    }
}

impl TryFrom<&[u8]> for InfoHash {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        <[u8; 32]>::try_from(bytes)
            .map(InfoHash)
            .map_err(|_| Error::InvalidLength {
                what: "infohash",
                expected: "32",
                actual: bytes.len(),
            })
    }
}

impl FromStr for InfoHash {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidHex { what: "infohash" })?;
        InfoHash::try_from(bytes.as_slice())
    }
}

impl fmt::Display for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let hex = "0f".repeat(32);
        let hash: InfoHash = hex.parse().unwrap();
        assert_eq!(hash.to_string(), hex, "Display must produce the parsed hex");
        assert!(
            "0f".repeat(20).parse::<InfoHash>().is_err(),
            "v1 (20-byte) infohashes are not accepted"
        );
    }
}
