use grt_core::{ObjectId, SignedManifest};
use grt_upstream::{PushResult, PushSpec};

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

    /// True if pushes to this remote sign new manifests, which needs the
    /// address's private key (`remote.<name>.torrentPublish`).
    fn publishes(&self) -> bool;

    /// Signs a new manifest with `specs` applied to the current one, and
    /// reports each ref's outcome. With `fast_forward_only`, unforced specs
    /// that would rewind a published ref are rejected, as a git server would.
    ///
    /// # Errors
    ///
    /// Returns an error if the current manifest cannot be determined.
    fn push(
        &mut self,
        specs: &[PushSpec],
        fast_forward_only: bool,
    ) -> Result<Vec<PushResult>, Error>;

    /// Remembers that `refs` were pushed to the swarm while the upstream was
    /// unreachable, for `git torrent reconcile` to replay.
    ///
    /// # Errors
    ///
    /// Returns an error if the record cannot be written.
    fn record_unsynced(&mut self, refs: &[String]) -> Result<(), Error>;

    /// Says on stderr which key signs this address's manifests, and checks
    /// that refs may come from them. Called before refs are served from a
    /// manifest instead of the upstream.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Untrusted`] if the repository trusts other keys, or
    /// pinned another key for this upstream.
    fn check_publisher(&mut self) -> Result<(), Error>;

    /// Pins this address's key for the upstream, unless it has a pin, after
    /// a fetch the live upstream answered (trust on first use).
    ///
    /// # Errors
    ///
    /// Returns an error if the pin cannot be stored.
    fn pin(&mut self) -> Result<(), Error>;
}
