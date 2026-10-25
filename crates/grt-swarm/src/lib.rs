//! Peer-to-peer layer: signed manifests on the Mainline DHT (BEP 44) and pack
//! transfer as BitTorrent v1 torrents (ADR-0006).
//!
//! [`ManifestDht`] publishes and resolves [`grt_core::SignedManifest`]s.
//! [`PackSwarm`] seeds packs and fetches them by infohash.

mod error;
mod manifest_dht;
mod pack_swarm;

pub use crate::error::Error;
pub use crate::manifest_dht::ManifestDht;
pub use crate::pack_swarm::{PackSwarm, SwarmConfig};
