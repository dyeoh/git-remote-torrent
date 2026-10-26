//! The git remote-helper protocol behind `git-remote-torrent`.
//!
//! Git starts the helper for `torrent::<url>` and `torrent://<key>` remotes and
//! talks to it over stdin/stdout; see `gitremote-helpers(7)`. Stdout carries
//! only protocol replies, so all diagnostics go to stderr.
//!
//! [`run`] speaks the protocol to any [`Remote`]. [`Router`] is the real
//! remote: it decides between the upstream and the [`Swarm`] per command.

mod dht_swarm;
mod error;
mod protocol;
mod remote;
mod router;
mod swarm;

pub use crate::dht_swarm::{DhtSwarm, Settings};
pub use crate::error::Error;
pub use crate::protocol::run;
pub use crate::remote::{FetchRef, Remote};
pub use crate::router::Router;
pub use crate::swarm::Swarm;
