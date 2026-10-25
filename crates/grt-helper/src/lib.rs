//! The git remote-helper protocol state machine behind `git-remote-torrent`.
//!
//! Git talks to the helper over stdin/stdout; see `gitremote-helpers(7)`.
//! Stdout is reserved for protocol replies, so all diagnostics go to stderr.
