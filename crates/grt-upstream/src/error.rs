use std::process::ExitStatus;

/// Errors talking to the upstream.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `git` could not be started.
    #[error("failed to run git: {0}")]
    Spawn(#[from] std::io::Error),
    /// A `git` command exited unsuccessfully. Its own stderr has already been
    /// shown to the user.
    #[error("`git {command}` failed ({status})")]
    GitFailed {
        /// The git subcommand.
        command: &'static str,
        /// How it exited.
        status: ExitStatus,
    },
    /// A `git` command printed something we could not parse.
    #[error("unexpected output from `git {command}`: {line:?}")]
    UnexpectedOutput {
        /// The git subcommand.
        command: &'static str,
        /// The offending line.
        line: String,
    },
    /// Fetching keys from a forge failed.
    #[error("forge request failed: {0}")]
    Forge(String),
    /// A push refspec is malformed.
    #[error("invalid push refspec {0:?}")]
    InvalidPushSpec(String),
}
