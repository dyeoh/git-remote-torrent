use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signature, VerifyingKey};

use crate::error::Error;

/// An Ed25519 public key identifying a manifest publisher.
///
/// In pure P2P mode this key, hex-encoded, is the repository address:
/// `torrent://<public-key>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    /// Length of an encoded public key in bytes.
    pub const LEN: usize = 32;

    /// Returns the 32-byte compressed Edwards point.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Verifies `signature` over `message`, rejecting weak keys and
    /// malleable signatures (`verify_strict`).
    pub(crate) fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<(), Error> {
        self.0
            .verify_strict(message, &Signature::from_bytes(signature))
            .map_err(|_| Error::BadSignature)
    }
}

impl From<VerifyingKey> for PublicKey {
    fn from(key: VerifyingKey) -> Self {
        PublicKey(key)
    }
}

impl TryFrom<&[u8]> for PublicKey {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        let bytes = <[u8; 32]>::try_from(bytes).map_err(|_| Error::InvalidLength {
            what: "public key",
            expected: "32",
            actual: bytes.len(),
        })?;
        VerifyingKey::from_bytes(&bytes)
            .map(PublicKey)
            .map_err(|_| Error::InvalidPublicKey)
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidHex { what: "public key" })?;
        PublicKey::try_from(bytes.as_slice())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.to_bytes()))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}
