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
    /// Git sent something this helper does not understand.
    #[error("protocol error: {0}")]
    Protocol(String),
}
