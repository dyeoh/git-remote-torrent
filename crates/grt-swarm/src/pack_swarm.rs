use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use grt_core::InfoHash;
use librqbit::spawn_utils::BlockingSpawner;
use librqbit::{
    AddTorrent, AddTorrentOptions, CreateTorrentOptions, DhtSessionConfig, ListenerMode,
    ListenerOptions, Session, SessionOptions, create_torrent,
};

use crate::error::Error;

/// How a [`PackSwarm`] connects to peers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SwarmConfig {
    /// TCP address to accept peers on.
    pub listen: SocketAddr,
    /// Find peers through the Mainline DHT, and on the local network
    /// through multicast Local Service Discovery (BEP 14). Off for tests.
    pub dht: bool,
    /// Ask the router to forward the listen port (UPnP), so peers outside a
    /// NAT can connect. Changes the router's configuration, so off by default.
    pub upnp: bool,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        SwarmConfig {
            listen: SocketAddr::from(([0, 0, 0, 0], 0)),
            dht: true,
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
            disable_local_service_discovery: !config.dht,
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

    /// Creates a torrent for the pack file at `pack`, seeds it from where it
    /// is, and returns its infohash.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Torrent`] if hashing or seeding fails.
    pub async fn seed(&self, pack: &Path) -> Result<InfoHash, Error> {
        let created = torrent_for(pack).await?;
        let info_hash = InfoHash::from(created.info_hash().0);
        let folder = pack
            .parent()
            .unwrap_or(Path::new("."))
            .to_string_lossy()
            .into_owned();
        let handle = self
            .session
            .add_torrent(
                AddTorrent::from_bytes(created.as_bytes().map_err(torrent_error)?),
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
        Ok(info_hash)
    }

    /// Downloads the pack with infohash `pack` into its own directory under
    /// `out_dir`, asking `peers` first, and returns the path of the pack file.
    ///
    /// The torrent's metadata comes from the peers (BEP 9), so the infohash
    /// from a manifest is all that is needed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Timeout`] if the download does not finish within
    /// `timeout`, or [`Error::Torrent`] if the engine fails.
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
            let handle = self
                .session
                .add_torrent(
                    AddTorrent::from_url(pack.to_string()),
                    Some(AddTorrentOptions {
                        overwrite: true,
                        output_folder: Some(folder.to_string_lossy().into_owned()),
                        initial_peers: Some(peers.to_vec()),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(torrent_error)?
                .into_handle()
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

/// Computes the BitTorrent v1 infohash a pack file would be seeded under,
/// without starting a session. The torrent is named after the file, so the
/// same bytes under a different file name give a different infohash.
///
/// # Errors
///
/// Returns [`Error::Torrent`] if the file cannot be read and hashed.
pub async fn info_hash_of(pack: &Path) -> Result<InfoHash, Error> {
    Ok(InfoHash::from(torrent_for(pack).await?.info_hash().0))
}

async fn torrent_for(pack: &Path) -> Result<librqbit::CreateTorrentResult, Error> {
    let name = pack.file_name().and_then(|name| name.to_str());
    create_torrent(
        pack,
        CreateTorrentOptions {
            name,
            ..Default::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .map_err(torrent_error)
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
