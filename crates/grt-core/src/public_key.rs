use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signature, VerifyingKey};
use ssh_key::public::{Ed25519PublicKey, KeyData};

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

    /// Parses an OpenSSH public key line (`ssh-ed25519 AAAA... [comment]`), as
    /// found in `authorized_keys`, `*.pub` files and forge `/<user>.keys` pages.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSshKey`] if the line is malformed or not an
    /// Ed25519 key, or [`Error::InvalidPublicKey`] if the key bytes are invalid.
    pub fn from_openssh(line: &str) -> Result<Self, Error> {
        let key =
            ssh_key::PublicKey::from_openssh(line.trim()).map_err(|_| Error::InvalidSshKey {
                reason: "not an OpenSSH public key line".to_owned(),
            })?;
        match key.key_data() {
            KeyData::Ed25519(Ed25519PublicKey(bytes)) => PublicKey::try_from(bytes.as_slice()),
            other => Err(Error::InvalidSshKey {
                reason: format!("{} keys are not supported; use Ed25519", other.algorithm()),
            }),
        }
    }

    /// Formats the key as an OpenSSH public key line with `comment`.
    ///
    /// # Panics
    ///
    /// Never in practice: encoding a 32-byte Ed25519 key into a `String` cannot fail.
    pub fn to_openssh(&self, comment: &str) -> String {
        let key =
            ssh_key::PublicKey::new(KeyData::Ed25519(Ed25519PublicKey(self.to_bytes())), comment);
        // Encoding an Ed25519 key into a String cannot fail.
        key.to_openssh().expect("Ed25519 public keys always encode")
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher_key::PublisherKey;

    #[test]
    fn openssh_round_trip() {
        let key = PublisherKey::from_seed([9; 32]).public_key();
        let line = key.to_openssh("alice@example");
        assert!(
            line.starts_with("ssh-ed25519 AAAA"),
            "OpenSSH Ed25519 line: {line}"
        );
        assert!(line.ends_with(" alice@example"), "comment is kept: {line}");
        assert_eq!(
            PublicKey::from_openssh(&line),
            Ok(key),
            "parse(format(k)) == k"
        );
    }

    #[test]
    fn rejects_other_ssh_keys() {
        let rsa = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQC7 nobody";
        assert!(PublicKey::from_openssh(rsa).is_err(), "RSA is not accepted");
        assert!(
            PublicKey::from_openssh("ssh-ed25519 !!!").is_err(),
            "bad base64"
        );
    }
}
