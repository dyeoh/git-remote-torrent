/// Errors from the DHT or the torrent engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A DHT node could not be started or a query failed.
    #[error("DHT: {0}")]
    Dht(String),
    /// Another publish for the same key and salt won a compare-and-swap race;
    /// resolve the latest manifest and try again.
    #[error("manifest conflict: {0}")]
    Conflict(String),
    /// The torrent engine failed.
    #[error("torrent: {0}")]
    Torrent(String),
    /// An HTTP mirror failed or served the wrong bytes.
    #[error("mirror: {0}")]
    Mirror(String),
    /// A download did not finish in time.
    #[error("timed out fetching pack {0}")]
    Timeout(grt_core::InfoHash),
    /// A manifest from the DHT failed validation.
    #[error(transparent)]
    Core(#[from] grt_core::Error),
    /// A local file operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
