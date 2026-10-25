//! The git remote-helper protocol behind `git-remote-torrent`.
//!
//! Git starts the helper for `torrent::<url>` and `torrent://<key>` remotes and
//! talks to it over stdin/stdout; see `gitremote-helpers(7)`. Stdout carries
//! only protocol replies, so all diagnostics go to stderr.

mod error;
mod protocol;

pub use crate::error::Error;
pub use crate::protocol::run;
