use std::net::Ipv4Addr;

use grt_core::{PublicKey, Seq, SignedManifest};
use mainline::async_dht::AsyncDht;
use mainline::errors::PutMutableError;
use mainline::{Dht, MutableItem};

use crate::error::Error;

/// Publishes and resolves signed manifests as BEP 44 mutable items.
#[derive(Debug, Clone)]
pub struct ManifestDht {
    dht: AsyncDht,
}

impl ManifestDht {
    /// Joins the public Mainline DHT as a client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dht`] if the UDP socket cannot be opened.
    pub fn public() -> Result<Self, Error> {
        let dht = Dht::client().map_err(|error| Error::Dht(error.to_string()))?;
        Ok(ManifestDht {
            dht: dht.as_async(),
        })
    }

    /// Joins a private DHT through `bootstrap` nodes (`host:port`), binding
    /// to `bind`. Used for tests and isolated networks.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dht`] if the UDP socket cannot be opened.
    pub fn with_bootstrap(bootstrap: &[String], bind: Ipv4Addr) -> Result<Self, Error> {
        let dht = Dht::builder()
            .bootstrap(bootstrap)
            .bind_address(bind)
            .build()
            .map_err(|error| Error::Dht(error.to_string()))?;
        Ok(ManifestDht {
            dht: dht.as_async(),
        })
    }

    /// Stores `signed` on the DHT.
    ///
    /// Pass the sequence number of the manifest this one replaces as `cas`
    /// so a concurrent publish is detected instead of silently overwritten.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Conflict`] if another publish raced this one, or
    /// [`Error::Dht`] if no node accepted the item.
    pub async fn publish(&self, signed: &SignedManifest, cas: Option<Seq>) -> Result<(), Error> {
        let item = MutableItem::new_signed_unchecked(
            signed.public_key().to_bytes(),
            *signed.signature(),
            signed.value(),
            signed.seq().get(),
            Some(signed.salt()),
        );
        match self.dht.put_mutable(item, cas.map(Seq::get)).await {
            Ok(_) => Ok(()),
            Err(PutMutableError::Concurrency(error)) => Err(Error::Conflict(error.to_string())),
            Err(error) => Err(Error::Dht(error.to_string())),
        }
    }

    /// Finds the most recent manifest published by `key` under `salt`, and
    /// verifies it.
    ///
    /// Returns `Ok(None)` if no node has one.
    ///
    /// # Errors
    ///
    /// Returns a [`grt_core::Error`] if the item fails signature verification
    /// or manifest validation.
    pub async fn resolve(
        &self,
        key: &PublicKey,
        salt: &[u8],
    ) -> Result<Option<SignedManifest>, Error> {
        let Some(item) = self
            .dht
            .get_mutable_most_recent(&key.to_bytes(), Some(salt))
            .await
        else {
            return Ok(None);
        };
        // mainline checks the signature too, but trust only grt-core's
        // verification and manifest validation.
        let signed = SignedManifest::verify(
            *key,
            salt,
            Seq::new(item.seq()),
            item.value(),
            item.signature(),
        )?;
        Ok(Some(signed))
    }
}
