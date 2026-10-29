//! Peer-to-peer layer: signed manifests on the Mainline DHT (BEP 44) and pack
//! transfer as BitTorrent hybrid v1/v2 torrents (ADR-0006, ADR-0008).
//!
//! [`ManifestDht`] publishes and resolves [`grt_core::SignedManifest`]s.
//! [`PackSwarm`] seeds packs and fetches them by infohash, and
//! [`fetch_from_mirror`] fetches them from HTTP mirrors, verified the same way.

mod error;
mod hybrid;
mod manifest_dht;
mod mirror;
mod pack_swarm;
mod store;

pub use crate::error::Error;
pub use crate::hybrid::{HybridTorrent, hybrid_torrent, piece_length};
pub use crate::manifest_dht::{ManifestDht, RESOLVE_ATTEMPTS};
pub use crate::mirror::{MAX_MIRROR_PACK_LEN, MirrorSource, fetch_from_mirror};
pub use crate::pack_swarm::{MAX_PACK_LEN, PackSwarm, SwarmConfig, hashes_of, verify_pack};
pub use crate::store::{fetched_manifest_path, packs_dir, stored_pack_path};
