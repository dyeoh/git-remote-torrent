use std::fmt;

use ed25519_dalek::{Signer, SigningKey};
use zeroize::{Zeroize, Zeroizing};

use crate::error::Error;
use crate::manifest::Manifest;
use crate::public_key::PublicKey;
use crate::signed_manifest::{Seq, SignedManifest};

/// The secret Ed25519 key that signs a repository's manifests.
///
/// The secret is zeroed when the key is dropped. `Debug` shows only the
/// public half.
#[derive(Clone)]
pub struct PublisherKey(SigningKey);

impl PublisherKey {
    /// Generates a new key from the operating system's random number generator.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Random`] if the system RNG is unavailable.
    pub fn generate() -> Result<Self, Error> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|_| Error::Random)?;
        Ok(PublisherKey::from_seed(*seed))
    }

    /// Rebuilds a key from its 32-byte secret seed, and wipes `seed`.
    pub fn from_seed(mut seed: [u8; 32]) -> Self {
        let key = PublisherKey(SigningKey::from_bytes(&seed));
        seed.zeroize();
        key
    }

    /// Returns the 32-byte secret seed, for storing the key. The copy is
    /// zeroed when dropped.
    pub fn to_seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.0.to_bytes())
    }

    /// Returns the matching public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey::from(self.0.verifying_key())
    }

    /// Signs `manifest` as BEP 44 mutable item number `seq` under `salt`.
    ///
    /// The salt lets one publisher key host several repositories; use an
    /// empty salt for a key's only repository.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TooLarge`] if the encoded manifest exceeds the BEP 44
    /// value limit, or [`Error::InvalidLength`] if `salt` is longer than 64 bytes.
    pub fn sign(
        &self,
        manifest: &Manifest,
        seq: Seq,
        salt: &[u8],
    ) -> Result<SignedManifest, Error> {
        SignedManifest::create(manifest, seq, salt, self.public_key(), |message| {
            self.0.sign(message).to_bytes()
        })
    }
}

impl From<[u8; 32]> for PublisherKey {
    fn from(seed: [u8; 32]) -> Self {
        PublisherKey::from_seed(seed)
    }
}

impl fmt::Debug for PublisherKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublisherKey")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_round_trip() {
        let key = PublisherKey::generate().unwrap();
        let restored = PublisherKey::from_seed(*key.to_seed());
        assert_eq!(
            key.public_key(),
            restored.public_key(),
            "same seed must give the same key"
        );
    }

    #[test]
    fn debug_hides_secret() {
        let key = PublisherKey::from_seed([7; 32]);
        let debug = format!("{key:?}");
        assert!(
            !debug.contains(&hex::encode([7u8; 32])),
            "Debug must not print the seed"
        );
        assert!(
            debug.contains(&key.public_key().to_string()),
            "Debug shows the public key"
        );
    }
}
