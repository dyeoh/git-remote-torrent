/// Errors that end a remote-helper session.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading from git or writing to it failed.
    #[error("I/O error talking to git: {0}")]
    Io(#[from] std::io::Error),
    /// The upstream operation failed.
    #[error(transparent)]
    Upstream(#[from] grt_upstream::Error),
    /// Resolving a manifest or transferring packs failed.
    #[error(transparent)]
    Swarm(#[from] grt_swarm::Error),
    /// A local git operation failed.
    #[error(transparent)]
    Git(#[from] grt_git::Error),
    /// A manifest or address failed validation.
    #[error(transparent)]
    Core(#[from] grt_core::Error),
    /// Neither the upstream nor the swarm can serve the request.
    #[error("{0}")]
    Unavailable(String),
    /// The manifest's key is not one this repository takes refs from.
    #[error("{0}")]
    Untrusted(String),
    /// Git sent something this helper does not understand.
    #[error("protocol error: {0}")]
    Protocol(String),
}
