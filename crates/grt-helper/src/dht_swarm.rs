use std::fmt::Write as _;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use grt_core::{InfoHash, ObjectId, PublicKey, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig};

use crate::error::Error;
use crate::swarm::Swarm;

/// How the helper reaches the swarm, from git config (`torrent.*`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Settings {
    /// DHT nodes to join instead of the public DHT (`torrent.dhtBootstrap`).
    pub bootstrap: Vec<String>,
    /// Peers to ask for packs before any found on the DHT (`torrent.peer`).
    pub peers: Vec<SocketAddr>,
    /// Find pack peers on the DHT (`torrent.peerDht`, default true).
    pub peer_dht: bool,
    /// Deadline for resolving the manifest and for each pack
    /// (`torrent.fetchTimeout`, in seconds, default 120).
    pub timeout: Duration,
}

impl Settings {
    /// Reads the settings from the config git uses in this environment.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unavailable`] if a value is malformed.
    pub fn from_config() -> Result<Self, Error> {
        let peers = grt_git::config_all("torrent.peer")?
            .iter()
            .map(|peer| {
                peer.parse()
                    .map_err(|_| invalid_config("torrent.peer", peer, "expected host:port"))
            })
            .collect::<Result<Vec<SocketAddr>, Error>>()?;
        let peer_dht = match last("torrent.peerDht")?.as_deref() {
            None | Some("true" | "yes" | "on" | "1") => true,
            Some("false" | "no" | "off" | "0") => false,
            Some(other) => {
                return Err(invalid_config(
                    "torrent.peerDht",
                    other,
                    "expected a boolean",
                ));
            }
        };
        let timeout =
            match last("torrent.fetchTimeout")? {
                None => Duration::from_secs(120),
                Some(secs) => Duration::from_secs(secs.parse().map_err(|_| {
                    invalid_config("torrent.fetchTimeout", &secs, "expected seconds")
                })?),
            };
        Ok(Settings {
            bootstrap: grt_git::config_all("torrent.dhtBootstrap")?,
            peers,
            peer_dht,
            timeout,
        })
    }
}

/// The swarm behind one `torrent://<key>[/<name>]` address.
///
/// The manifest is resolved once per session. The last manifest fetched is
/// kept under `.git/torrent/fetched/`, so a repository can still be fetched
/// when the DHT is unreachable, and an older manifest served by the DHT is
/// never applied over a newer one (rollback).
pub struct DhtSwarm {
    runtime: tokio::runtime::Runtime,
    repo: Option<Repo>,
    key: PublicKey,
    salt: Vec<u8>,
    settings: Settings,
    /// Set once the manifest has been looked up, found or not.
    resolved: bool,
    manifest: Option<SignedManifest>,
}

impl DhtSwarm {
    /// Serves the address `key`/`salt` into `repo`. Without a repository,
    /// as for `git ls-remote` outside one, refs can be listed but not fetched.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the async runtime cannot start.
    pub fn new(
        repo: Option<Repo>,
        key: PublicKey,
        salt: &[u8],
        settings: Settings,
    ) -> Result<Self, Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(DhtSwarm {
            runtime,
            repo,
            key,
            salt: salt.to_vec(),
            settings,
            resolved: false,
            manifest: None,
        })
    }

    async fn resolve(&self) -> Result<Option<SignedManifest>, Error> {
        let dht = ManifestDht::connect(&self.settings.bootstrap)?;
        tokio::time::timeout(self.settings.timeout, dht.resolve(&self.key, &self.salt))
            .await
            .map_err(|_| Error::Unavailable("timed out resolving the manifest".to_owned()))?
            .map_err(Error::from)
    }

    /// Downloads and indexes every pack of the chain not applied before.
    async fn apply_chain(&self, repo: &Repo, packs: &[InfoHash]) -> Result<usize, Error> {
        let torrent_dir = repo.git_dir().join("torrent");
        let mut config = SwarmConfig::default();
        config.dht = self.settings.peer_dht;
        let swarm = PackSwarm::start(&torrent_dir.join("fetch-session"), config).await?;
        let incoming = torrent_dir.join("incoming");
        let applied = torrent_dir.join("applied");
        let result = async {
            fs::create_dir_all(&applied)?;
            let mut fetched = 0;
            // Oldest first: each pack's links resolve against the ones before it.
            for &pack in packs {
                let marker = applied.join(pack.to_string());
                if marker.exists() {
                    continue;
                }
                let file = swarm
                    .fetch(pack, &self.settings.peers, &incoming, self.settings.timeout)
                    .await?;
                repo.index_pack(&file)?;
                fs::write(&marker, b"")?;
                fetched += 1;
            }
            Ok::<usize, Error>(fetched)
        }
        .await;
        swarm.stop().await;
        // Downloads are copied into the object store by index-pack.
        if incoming.exists() {
            fs::remove_dir_all(&incoming)?;
        }
        result
    }

    fn cache_path(&self, repo: &Repo) -> PathBuf {
        let mut name = self.key.to_string();
        if !self.salt.is_empty() {
            name.push('.');
            for byte in &self.salt {
                let _ = write!(name, "{byte:02x}");
            }
        }
        repo.git_dir().join("torrent").join("fetched").join(name)
    }

    /// The remembered manifest, if it is for this address and still verifies.
    fn cached(&self) -> Option<SignedManifest> {
        let bytes = fs::read(self.cache_path(self.repo.as_ref()?)).ok()?;
        let cached = SignedManifest::decode(&bytes).ok()?;
        (cached.public_key() == self.key && cached.salt() == self.salt).then_some(cached)
    }
}

impl Swarm for DhtSwarm {
    fn manifest(&mut self) -> Result<Option<SignedManifest>, Error> {
        if self.resolved {
            return Ok(self.manifest.clone());
        }
        let cached = self.cached();
        let manifest = match (self.runtime.block_on(self.resolve()), cached) {
            (Ok(Some(found)), Some(cached)) if cached.seq() > found.seq() => {
                eprintln!(
                    "warning: the DHT returned manifest #{}, older than #{} fetched before; \
                     using #{}",
                    found.seq().get(),
                    cached.seq().get(),
                    cached.seq().get()
                );
                Some(cached)
            }
            (Ok(Some(found)), _) => Some(found),
            (Ok(None), Some(cached)) => {
                eprintln!(
                    "warning: no manifest found on the DHT; using manifest #{} fetched before",
                    cached.seq().get()
                );
                Some(cached)
            }
            (Ok(None), None) => None,
            (Err(error), Some(cached)) => {
                eprintln!(
                    "warning: the DHT is unavailable ({error}); using manifest #{} fetched before",
                    cached.seq().get()
                );
                Some(cached)
            }
            (Err(error), None) => return Err(error),
        };
        self.resolved = true;
        self.manifest.clone_from(&manifest);
        Ok(manifest)
    }

    fn fetch(&mut self, manifest: &SignedManifest, wants: &[ObjectId]) -> Result<(), Error> {
        let repo = self.repo.as_ref().ok_or_else(|| {
            Error::Unavailable("fetching from the swarm needs a local repository".to_owned())
        })?;
        let fetched = self
            .runtime
            .block_on(self.apply_chain(repo, manifest.manifest().packs()))?;
        for want in wants {
            if !repo.contains(want)? {
                return Err(Error::Unavailable(format!(
                    "the swarm's packs for manifest #{} do not contain {want}",
                    manifest.seq().get()
                )));
            }
        }
        let path = self.cache_path(repo);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, manifest.encode())?;
        eprintln!(
            "Fetched {fetched} pack(s) of manifest #{} from the swarm.",
            manifest.seq().get()
        );
        Ok(())
    }
}

fn last(key: &str) -> Result<Option<String>, Error> {
    Ok(grt_git::config_all(key)?.pop())
}

fn invalid_config(key: &str, value: &str, expected: &str) -> Error {
    Error::Unavailable(format!("invalid {key} {value:?}: {expected}"))
}
