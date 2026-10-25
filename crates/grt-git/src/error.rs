use std::process::ExitStatus;

/// Errors running git.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// `git` could not be started.
    #[error("failed to run git: {0}")]
    Spawn(#[from] std::io::Error),
    /// A git command exited unsuccessfully.
    #[error("`git {command}` failed ({status}): {stderr}")]
    Failed {
        /// The git subcommand.
        command: String,
        /// How it exited.
        status: ExitStatus,
        /// What it printed to stderr.
        stderr: String,
    },
    /// Git reported a ref name or object id that fails validation.
    #[error(transparent)]
    Invalid(#[from] grt_core::Error),
    /// A git command printed something we could not parse.
    #[error("unexpected output from `git {command}`: {line:?}")]
    UnexpectedOutput {
        /// The git subcommand.
        command: String,
        /// The offending line.
        line: String,
    },
}
