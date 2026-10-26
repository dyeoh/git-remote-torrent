use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use grt_core::{PublicKey, Seq, SignedManifest};
use mainline::async_dht::AsyncDht;
use mainline::errors::PutMutableError;
use mainline::{Dht, MutableItem};

use crate::error::Error;

/// Lookups made before `resolve` concludes that nothing is published.
pub const RESOLVE_ATTEMPTS: u32 = 3;

/// Pause between lookups that found nothing.
const RESOLVE_RETRY_DELAY: Duration = Duration::from_secs(1);

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

    /// Joins the public DHT if `bootstrap` is empty, and otherwise the
    /// private DHT those nodes belong to. A network of loopback nodes gets a
    /// loopback socket, since it cannot reach any other address.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dht`] if the UDP socket cannot be opened.
    pub fn connect(bootstrap: &[String]) -> Result<Self, Error> {
        if bootstrap.is_empty() {
            return ManifestDht::public();
        }
        let all_loopback = bootstrap.iter().all(|node| {
            node.parse::<SocketAddr>()
                .is_ok_and(|addr| addr.ip().is_loopback())
        });
        let bind = if all_loopback {
            Ipv4Addr::LOCALHOST
        } else {
            Ipv4Addr::UNSPECIFIED
        };
        ManifestDht::with_bootstrap(bootstrap, bind)
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
            bep44_salt(signed.salt()),
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
    /// Returns `Ok(None)` if no node has one after [`RESOLVE_ATTEMPTS`]
    /// lookups.
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
        // A lookup from a client that has only just joined can end before it
        // reaches the nodes holding the item; each retry starts from a
        // fuller routing table.
        let mut found = None;
        for attempt in 0..RESOLVE_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(RESOLVE_RETRY_DELAY).await;
            }
            found = self
                .dht
                .get_mutable_most_recent(&key.to_bytes(), bep44_salt(salt))
                .await;
            if found.is_some() {
                break;
            }
        }
        let Some(item) = found else {
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

/// BEP 44 treats an empty salt as no salt: it is left out of both the
/// signature and the DHT target. `mainline` would sign `Some(b"")` as
/// `4:salt0:`, which matches neither grt-core's signature nor other clients.
fn bep44_salt(salt: &[u8]) -> Option<&[u8]> {
    (!salt.is_empty()).then_some(salt)
}
