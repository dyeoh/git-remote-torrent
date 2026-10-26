use grt_core::{ObjectId, SignedManifest};

use crate::error::Error;

/// The peer-to-peer side of a remote: one address's manifest and its packs.
/// [`crate::DhtSwarm`] is the real one; tests substitute fakes.
pub trait Swarm {
    /// The newest verified manifest for this address, or `None` if none is
    /// published or remembered.
    ///
    /// # Errors
    ///
    /// Returns an error if the DHT cannot be queried and no manifest is
    /// remembered.
    fn manifest(&mut self) -> Result<Option<SignedManifest>, Error>;

    /// Applies `manifest`'s pack chain to the local repository and checks
    /// that every object in `wants` arrived.
    ///
    /// # Errors
    ///
    /// Returns an error if a pack cannot be downloaded or indexed, or the
    /// chain does not contain `wants`.
    fn fetch(&mut self, manifest: &SignedManifest, wants: &[ObjectId]) -> Result<(), Error>;
}
