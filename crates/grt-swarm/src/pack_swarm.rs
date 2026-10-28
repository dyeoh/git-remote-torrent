use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use grt_core::{InfoHash, InfoHashV2};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ListenerMode,
    ListenerOptions, Session, SessionOptions,
};

use crate::error::Error;
use crate::hybrid::{HybridTorrent, PACK_FILE, hybrid_torrent};

/// Largest pack accepted from peers or mirrors, in bytes: 4 GiB. A pack's
/// infohash is signed, so only the publisher could make one larger, but a
/// cloner shouldn't fill their disk on the publisher's say-so.
pub const MAX_PACK_LEN: u64 = 4 << 30;

/// How a [`PackSwarm`] connects to peers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SwarmConfig {
    /// TCP address to accept peers on.
    pub listen: SocketAddr,
    /// Find peers through the Mainline DHT. Off for tests.
    pub dht: bool,
    /// While `dht` is on, also find peers on the local network through
    /// multicast Local Service Discovery (BEP 14), which tells the LAN which
    /// packs are fetched or seeded.
    pub local_discovery: bool,
    /// Ask the router to forward the listen port (UPnP), so peers outside a
    /// NAT can connect. Changes the router's configuration, so off by default.
    pub upnp: bool,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        SwarmConfig {
            listen: SocketAddr::from(([0, 0, 0, 0], 0)),
            dht: true,
            local_discovery: true,
            upnp: false,
        }
    }
}

/// Seeds and fetches packs as BitTorrent v1 torrents.
pub struct PackSwarm {
    session: Arc<Session>,
}

impl PackSwarm {
    /// Starts a torrent session that keeps its state under `dir`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Torrent`] if the session cannot start, for example
    /// because the listen address is taken.
    pub async fn start(dir: &Path, config: SwarmConfig) -> Result<Self, Error> {
        let options = SessionOptions {
            // librqbit's DHT persistence stores the bound port in one state
            // file per user, so a second session (a clone while `seed` runs)
            // reuses it and fails to bind. Without it, each session picks a
            // free port and leaves no global state behind.
            dht: config.dht.then(|| DhtSessionConfig {
                persistence: None,
                ..Default::default()
            }),
            disable_trackers: true,
            // Peers found on the DHT are listed under their public address,
            // which machines behind the same NAT often cannot connect back to.
            disable_local_service_discovery: !(config.dht && config.local_discovery),
            fastresume: false,
            persistence: None,
            listen: Some(ListenerOptions {
                mode: ListenerMode::TcpOnly,
                listen_addr: config.listen,
                enable_upnp_port_forwarding: config.upnp,
                ..Default::default()
            }),
            ..Default::default()
        };
        let session = Session::new_with_opts(dir.to_path_buf(), options)
            .await
            .map_err(torrent_error)?;
        Ok(PackSwarm { session })
    }

    /// Seeds the pack file at `pack` from where it is, as a hybrid torrent
    /// (ADR-0008), and returns its v1 infohash. The file must be named `pack`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Torrent`] if the file is named otherwise, or hashing
    /// or seeding fails.
    pub async fn seed(&self, pack: &Path) -> Result<InfoHash, Error> {
        // The torrent names its file `pack`; librqbit would otherwise wait
        // forever to download a file that is not where it looks.
        if pack.file_name().and_then(|name| name.to_str()) != Some(PACK_FILE) {
            return Err(Error::Torrent(format!(
                "{} must be named `{PACK_FILE}` to be seeded",
                pack.display()
            )));
        }
        let torrent = hashes_of(pack).await?;
        let folder = pack
            .parent()
            .unwrap_or(Path::new("."))
            .to_string_lossy()
            .into_owned();
        let handle = self
            .session
            .add_torrent(
                AddTorrent::from_bytes(torrent.bytes),
                Some(AddTorrentOptions {
                    overwrite: true,
                    output_folder: Some(folder),
                    ..Default::default()
                }),
            )
            .await
            .map_err(torrent_error)?
            .into_handle()
            .ok_or_else(|| Error::Torrent("seeding returned no torrent handle".to_owned()))?;
        // Completes once the existing file has been verified against the torrent.
        handle.wait_until_completed().await.map_err(torrent_error)?;
        Ok(torrent.v1)
    }

    /// Downloads the pack with infohash `pack` into its own directory under
    /// `out_dir`, asking `peers` first, and returns the path of the pack file.
    ///
    /// The torrent's metadata comes from the peers (BEP 9), so the infohash
    /// from a manifest is all that is needed. It is checked before anything
    /// else is downloaded: a pack larger than [`MAX_PACK_LEN`] is refused.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Timeout`] if the download does not finish within
    /// `timeout`, [`Error::TooLarge`] if the pack exceeds [`MAX_PACK_LEN`],
    /// or [`Error::Torrent`] if the engine fails.
    pub async fn fetch(
        &self,
        pack: InfoHash,
        peers: &[SocketAddr],
        out_dir: &Path,
        timeout: Duration,
    ) -> Result<PathBuf, Error> {
        let folder = out_dir.join(pack.to_string());
        fs::create_dir_all(&folder)?;
        // For a bare infohash, add_torrent itself fetches the metadata from
        // peers, so the deadline must cover it as well as the download.
        let download = async {
            // First only the metadata, to check the size before downloading.
            let listed = self
                .session
                .add_torrent(
                    AddTorrent::from_url(pack.to_string()),
                    Some(AddTorrentOptions {
                        list_only: true,
                        initial_peers: Some(peers.to_vec()),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(torrent_error)?;
            let handle = match listed {
                AddTorrentResponse::ListOnly(listed) => {
                    let len = listed.info.lengths().total_length();
                    if len > MAX_PACK_LEN {
                        return Err(Error::TooLarge { pack, len });
                    }
                    let mut known_peers = peers.to_vec();
                    known_peers.extend(listed.seen_peers);
                    self.session
                        .add_torrent(
                            AddTorrent::from_bytes(listed.torrent_bytes),
                            Some(AddTorrentOptions {
                                overwrite: true,
                                output_folder: Some(folder.to_string_lossy().into_owned()),
                                initial_peers: Some(known_peers),
                                ..Default::default()
                            }),
                        )
                        .await
                        .map_err(torrent_error)?
                        .into_handle()
                }
                // This session already seeds or fetches the pack.
                other => other.into_handle(),
            }
            .ok_or_else(|| Error::Torrent("fetch returned no torrent handle".to_owned()))?;
            handle.wait_until_completed().await.map_err(torrent_error)
        };
        tokio::time::timeout(timeout, download)
            .await
            .map_err(|_| Error::Timeout(pack))??;
        single_file(&folder)
    }

    /// The address peers can connect to, once listening.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.session.listen_addr()
    }

    /// Stops every torrent and closes the session.
    pub async fn stop(&self) {
        self.session.stop().await;
    }
}

/// Computes the v1 and v2 infohashes a pack file is seeded under, without
/// starting a session. The torrent is always named `pack`, so they depend
/// only on the file's bytes.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file cannot be read.
pub async fn hashes_of(pack: &Path) -> Result<HybridTorrent, Error> {
    let pack = pack.to_path_buf();
    tokio::task::spawn_blocking(move || hybrid_torrent(&pack))
        .await
        .map_err(torrent_error)?
}

/// Checks that the file at `pack` hashes to `v1` and, if given, `v2`.
///
/// # Errors
///
/// Returns [`Error::Mismatch`] naming the hash that differs.
pub async fn verify_pack(pack: &Path, v1: InfoHash, v2: Option<InfoHashV2>) -> Result<(), Error> {
    let found = hashes_of(pack).await?;
    if found.v1 != v1 {
        return Err(Error::Mismatch(format!(
            "{} has v1 infohash {}, not {v1}",
            pack.display(),
            found.v1
        )));
    }
    if let Some(v2) = v2
        && found.v2 != v2
    {
        return Err(Error::Mismatch(format!(
            "{} has SHA-256 (v2) infohash {}, not {v2}",
            pack.display(),
            found.v2
        )));
    }
    Ok(())
}

/// A pack torrent holds exactly one file.
fn single_file(folder: &Path) -> Result<PathBuf, Error> {
    let mut files = fs::read_dir(folder)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<PathBuf>, _>>()?;
    match (files.pop(), files.is_empty()) {
        (Some(file), true) => Ok(file),
        _ => Err(Error::Torrent(format!(
            "expected exactly one file in {}",
            folder.display()
        ))),
    }
}

fn torrent_error(error: impl std::fmt::Display) -> Error {
    Error::Torrent(format!("{error:#}"))
}
