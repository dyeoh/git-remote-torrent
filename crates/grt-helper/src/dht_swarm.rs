use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{fs, io};

use grt_core::{Manifest, ObjectId, PublicKey, RefName, SignedManifest};
use grt_git::Repo;
use grt_swarm::{
    ManifestDht, MirrorSource, PackSwarm, SwarmConfig, fetch_from_mirror, verify_pack,
};
use grt_upstream::{PushResult, PushSpec, PushStatus};

use crate::error::Error;
use crate::swarm::Swarm;
use crate::trust;

/// Refs pushed to the swarm while the upstream was down, as `<remote> <ref>`
/// lines under `.git/torrent/`; read by `git torrent reconcile`.
const UNSYNCED_FILE: &str = "unsynced";

/// How long to wait for peers before trying HTTP mirrors.
const SWARM_TIMEOUT_WITH_MIRRORS: Duration = Duration::from_secs(30);

/// Packs downloaded in one fetch, by source.
#[derive(Debug, Default)]
struct Fetched {
    from_peers: usize,
    from_mirrors: usize,
}

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
    /// Also find them on the local network by multicast, while `peer_dht`
    /// is on (`torrent.localDiscovery`, default true).
    pub local_discovery: bool,
    /// HTTP mirrors to try after peers, besides those the manifest lists
    /// (`torrent.mirror`).
    pub mirrors: Vec<String>,
    /// Let the manifest's mirrors reach loopback and private addresses
    /// (`torrent.allowPrivateMirrors`, default false).
    pub allow_private_mirrors: bool,
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
        let peer_dht = boolean("torrent.peerDht", true)?;
        let local_discovery = boolean("torrent.localDiscovery", true)?;
        let allow_private_mirrors = boolean("torrent.allowPrivateMirrors", false)?;
        let timeout =
            match last("torrent.fetchTimeout")? {
                None => Duration::from_secs(120),
                Some(secs) => Duration::from_secs(secs.parse().map_err(|_| {
                    invalid_config("torrent.fetchTimeout", &secs, "expected seconds")
                })?),
            };
        let mirrors = grt_git::config_all("torrent.mirror")?;
        for mirror in &mirrors {
            if !(mirror.starts_with("https://") || mirror.starts_with("http://")) {
                return Err(invalid_config(
                    "torrent.mirror",
                    mirror,
                    "expected an http:// or https:// URL",
                ));
            }
        }
        Ok(Settings {
            bootstrap: grt_git::config_all("torrent.dhtBootstrap")?,
            peers,
            peer_dht,
            local_discovery,
            mirrors,
            allow_private_mirrors,
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
    publishes: bool,
    remote: Option<String>,
    upstream: Option<String>,
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
            publishes: false,
            remote: None,
            upstream: None,
            manifest: None,
        })
    }

    /// Lets pushes sign new manifests for this address with the signing key
    /// (`git torrent key`), which must be the address's key.
    #[must_use]
    pub fn publishing(mut self, publishes: bool) -> Self {
        self.publishes = publishes;
        self
    }

    /// Names the git remote this swarm serves, which `record_unsynced`
    /// needs.
    #[must_use]
    pub fn for_remote(mut self, remote: Option<&str>) -> Self {
        self.remote = remote.map(str::to_owned);
        self
    }

    /// Names the upstream of a hybrid remote, which the publisher key is
    /// pinned for.
    #[must_use]
    pub fn for_upstream(mut self, upstream: Option<&str>) -> Self {
        self.upstream = upstream.map(str::to_owned);
        self
    }

    fn repo(&self) -> Result<&Repo, Error> {
        self.repo
            .as_ref()
            .ok_or_else(|| Error::Unavailable("this needs a local repository".to_owned()))
    }

    async fn resolve(&self) -> Result<Option<SignedManifest>, Error> {
        let dht = ManifestDht::connect(&self.settings.bootstrap)?;
        tokio::time::timeout(self.settings.timeout, dht.resolve(&self.key, &self.salt))
            .await
            .map_err(|_| Error::Unavailable("timed out resolving the manifest".to_owned()))?
            .map_err(Error::from)
    }

    /// Downloads and indexes every pack of the chain not applied before,
    /// from peers first and then from HTTP mirrors.
    /// The manifest's mirrors, then the user's, with who chose each.
    fn mirrors<'a>(&'a self, manifest: &'a Manifest) -> Vec<(&'a str, MirrorSource)> {
        let from_manifest = if self.settings.allow_private_mirrors {
            MirrorSource::User
        } else {
            MirrorSource::Manifest
        };
        let mut mirrors: Vec<(&str, MirrorSource)> = manifest
            .mirrors()
            .iter()
            .map(|mirror| (mirror.as_str(), from_manifest))
            .collect();
        mirrors.extend(
            self.settings
                .mirrors
                .iter()
                .map(|mirror| (mirror.as_str(), MirrorSource::User)),
        );
        mirrors
    }

    async fn apply_chain(&self, repo: &Repo, manifest: &Manifest) -> Result<Fetched, Error> {
        let torrent_dir = repo.git_dir().join("torrent");
        let mirrors = self.mirrors(manifest);
        // Without peers to ask, go straight to the mirrors.
        let swarm = if self.settings.peers.is_empty() && !self.settings.peer_dht {
            None
        } else {
            let mut config = SwarmConfig::default();
            config.dht = self.settings.peer_dht;
            config.local_discovery = self.settings.local_discovery;
            Some(PackSwarm::start(&torrent_dir.join("fetch-session"), config).await?)
        };
        // With a mirror to fall back on, don't wait long for absent peers.
        let swarm_timeout = if mirrors.is_empty() {
            self.settings.timeout
        } else {
            self.settings.timeout.min(SWARM_TIMEOUT_WITH_MIRRORS)
        };
        let incoming = torrent_dir.join("incoming");
        let applied = torrent_dir.join("applied");
        let result = async {
            fs::create_dir_all(&applied)?;
            let mut fetched = Fetched::default();
            // Oldest first: each pack's links resolve against the ones before it.
            for (index, &pack) in manifest.packs().iter().enumerate() {
                // Present for chains signed since 0.4 (ADR-0008).
                let sha256 = manifest.packs_v2().map(|v2| v2[index]);
                let marker = applied.join(pack.to_string());
                if marker.exists() {
                    continue;
                }
                let mut failures = Vec::new();
                let mut file = None;
                if let Some(swarm) = &swarm {
                    match swarm
                        .fetch(pack, &self.settings.peers, &incoming, swarm_timeout)
                        .await
                    {
                        // Peers check SHA-1 pieces; check SHA-256 here too.
                        Ok(path) => match verify_pack(&path, pack, sha256).await {
                            Ok(()) => {
                                fetched.from_peers += 1;
                                file = Some(path);
                            }
                            Err(error) => failures.push(format!("peers: {error}")),
                        },
                        Err(error) => failures.push(format!("peers: {error}")),
                    }
                }
                for &(mirror, source) in &mirrors {
                    if file.is_some() {
                        break;
                    }
                    match fetch_from_mirror(
                        mirror,
                        source,
                        pack,
                        sha256,
                        &incoming,
                        self.settings.timeout,
                    )
                    .await
                    {
                        Ok(path) => {
                            fetched.from_mirrors += 1;
                            file = Some(path);
                        }
                        Err(error) => failures.push(error.to_string()),
                    }
                }
                let Some(file) = file else {
                    return Err(Error::Unavailable(format!(
                        "no source has pack {pack} ({})",
                        if failures.is_empty() {
                            "no peers or mirrors configured".to_owned()
                        } else {
                            failures.join("; ")
                        }
                    )));
                };
                repo.index_pack(&file)?;
                fs::write(&marker, b"")?;
            }
            Ok::<Fetched, Error>(fetched)
        }
        .await;
        if let Some(swarm) = swarm {
            swarm.stop().await;
        }
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

    /// The newest remembered manifest for this address that still verifies:
    /// the last one fetched, or the last one signed here by its publisher.
    fn cached(&self) -> Option<SignedManifest> {
        let repo = self.repo.as_ref()?;
        let signed_here = repo.git_dir().join("torrent").join("manifest");
        [self.cache_path(repo), signed_here]
            .iter()
            .filter_map(|path| SignedManifest::decode(&fs::read(path).ok()?).ok())
            .filter(|cached| cached.public_key() == self.key && cached.salt() == self.salt)
            .max_by_key(SignedManifest::seq)
    }

    /// Checks one push spec against the published refs. Returns the
    /// `git torrent sign` arguments that apply it, or why it is rejected.
    fn sign_args(
        repo: &Repo,
        spec: &PushSpec,
        published: &BTreeMap<RefName, ObjectId>,
        fast_forward_only: bool,
    ) -> Result<[String; 2], String> {
        let dst: RefName = spec.dst.parse().map_err(|error| format!("{error}"))?;
        if spec.src.is_empty() {
            return Ok(["--delete".to_owned(), dst.to_string()]);
        }
        let new = repo
            .rev_parse(&spec.src)
            .map_err(|_| format!("cannot resolve {}", spec.src))?;
        if fast_forward_only
            && !spec.force
            && let Some(old) = published.get(&dst)
            && *old != new
        {
            match repo.is_ancestor(old, &new) {
                Ok(true) => {}
                Ok(false) => return Err("non-fast-forward".to_owned()),
                // The published tip is not here: the remote has history we lack.
                Err(_) => return Err("fetch first".to_owned()),
            }
        }
        Ok(["--update".to_owned(), format!("{dst}={new}")])
    }

    /// True if `manifest` is the last one signed in this repository, rather
    /// than one published from another machine.
    fn signed_here(&self, repo: &Repo, manifest: &SignedManifest) -> bool {
        let path = repo.git_dir().join("torrent").join("manifest");
        fs::read(path)
            .ok()
            .and_then(|bytes| SignedManifest::decode(&bytes).ok())
            .is_some_and(|own| {
                own.public_key() == self.key
                    && own.salt() == self.salt
                    && own.seq() == manifest.seq()
                    && own.value() == manifest.value()
            })
    }

    /// True if `git torrent seed` is running for this repository.
    fn seeder_running(repo: &Repo) -> bool {
        let Ok(pid) = fs::read_to_string(repo.git_dir().join("torrent").join("seed.pid")) else {
            return false;
        };
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
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
            .block_on(self.apply_chain(repo, manifest.manifest()))?;
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
        let mirrored = if fetched.from_mirrors == 0 {
            String::new()
        } else {
            format!(", {} from HTTP mirrors", fetched.from_mirrors)
        };
        eprintln!(
            "Fetched {} pack(s) of manifest #{} from the swarm{mirrored}.",
            fetched.from_peers + fetched.from_mirrors,
            manifest.seq().get()
        );
        Ok(())
    }

    fn record_unsynced(&mut self, refs: &[String]) -> Result<(), Error> {
        let remote = self.remote.as_deref().ok_or_else(|| {
            Error::Unavailable("an unnamed remote cannot be reconciled later".to_owned())
        })?;
        let path = self.repo()?.git_dir().join("torrent").join(UNSYNCED_FILE);
        let mut lines: BTreeSet<String> = fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        lines.extend(refs.iter().map(|name| format!("{remote} {name}")));
        let mut contents = lines.into_iter().collect::<Vec<String>>().join("\n");
        contents.push('\n');
        fs::write(&path, contents)?;
        Ok(())
    }

    fn publishes(&self) -> bool {
        self.publishes
    }

    fn check_publisher(&mut self) -> Result<(), Error> {
        trust::check(
            self.repo.as_ref().map(Repo::git_dir),
            &self.key,
            self.upstream.as_deref(),
        )
    }

    fn pin(&mut self) -> Result<(), Error> {
        match (&self.repo, &self.upstream) {
            (Some(repo), Some(upstream)) => trust::pin(repo.git_dir(), &self.key, upstream),
            _ => Ok(()),
        }
    }

    fn push(
        &mut self,
        specs: &[PushSpec],
        fast_forward_only: bool,
    ) -> Result<Vec<PushResult>, Error> {
        let newest = self.manifest()?;
        let repo = self.repo()?;
        // One machine publishes an address: its pack chain lives only there,
        // so a manifest signed elsewhere cannot be extended here.
        if let Some(newest) = &newest
            && !self.signed_here(repo, newest)
        {
            return Ok(specs
                .iter()
                .map(|spec| PushResult {
                    dst: spec.dst.clone(),
                    status: PushStatus::Rejected(format!(
                        "fetch first: manifest #{} of this address was signed on another \
                         machine, and only one machine can publish an address",
                        newest.seq().get()
                    )),
                })
                .collect());
        }
        let published = newest
            .map(|current| current.manifest().refs().clone())
            .unwrap_or_default();
        let mut command = Command::new("git");
        command.args(["torrent", "sign"]);
        if !self.salt.is_empty() {
            command.args(["--name", &String::from_utf8_lossy(&self.salt)]);
        }
        let mut checked = Vec::new();
        for spec in specs {
            let outcome = DhtSwarm::sign_args(repo, spec, &published, fast_forward_only);
            if let Ok(args) = &outcome {
                command.args(args);
            }
            checked.push((spec.dst.clone(), outcome));
        }
        let signed = if checked.iter().any(|(_, outcome)| outcome.is_ok()) {
            // Stdout belongs to the protocol; sign's report goes to the user.
            let status = command
                .stdin(Stdio::null())
                .stdout(io::stderr().as_fd().try_clone_to_owned()?)
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("`git torrent sign` failed ({status})"))
            }
        } else {
            Ok(())
        };
        if signed.is_ok() && checked.iter().any(|(_, outcome)| outcome.is_ok()) {
            if DhtSwarm::seeder_running(repo) {
                eprintln!("The running `git torrent seed` will publish the new manifest.");
            } else {
                eprintln!("Run `git torrent seed` to publish the new manifest to the swarm.");
            }
        }
        // The next lookup must see the manifest just signed.
        self.resolved = false;
        Ok(checked
            .into_iter()
            .map(|(dst, outcome)| PushResult {
                dst,
                status: match (outcome, &signed) {
                    (Ok(_), Ok(())) => PushStatus::Ok,
                    (Ok(_), Err(reason)) => PushStatus::Rejected(reason.clone()),
                    (Err(reason), _) => PushStatus::Rejected(reason),
                },
            })
            .collect())
    }
}

fn last(key: &str) -> Result<Option<String>, Error> {
    Ok(grt_git::config_all(key)?.pop())
}

/// A boolean setting, `default` when unset.
fn boolean(key: &str, default: bool) -> Result<bool, Error> {
    match last(key)?.as_deref() {
        None => Ok(default),
        Some("true" | "yes" | "on" | "1") => Ok(true),
        Some("false" | "no" | "off" | "0") => Ok(false),
        Some(other) => Err(invalid_config(key, other, "expected a boolean")),
    }
}

fn invalid_config(key: &str, value: &str, expected: &str) -> Error {
    Error::Unavailable(format!("invalid {key} {value:?}: {expected}"))
}
