//! Downloads a manifest's packs from HTTP mirrors and peers, each checked
//! against both of the manifest's infohashes.

use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::bail;
use grt_core::{InfoHash, InfoHashV2, Manifest};
use grt_swarm::{MirrorSource, PackSwarm, SwarmConfig, fetch_from_mirror, verify_pack};

/// Deadline for each pack download.
pub(crate) const PACK_TIMEOUT: Duration = Duration::from_secs(120);

/// Where packs not found on a mirror are asked for.
pub(crate) enum Peers<'a> {
    /// A session that is already running, such as a seeder's. Packs are
    /// downloaded into `into`, where the session goes on seeding them.
    Running {
        swarm: &'a PackSwarm,
        peers: &'a [SocketAddr],
        into: PathBuf,
    },
    /// A session started in `dir` the first time a mirror lacks a pack.
    /// Packs are downloaded under `dir`.
    Lazy {
        dir: PathBuf,
        config: SwarmConfig,
        peers: &'a [SocketAddr],
        started: Option<PackSwarm>,
    },
}

impl Peers<'_> {
    async fn swarm(&mut self) -> anyhow::Result<(&PackSwarm, &[SocketAddr], PathBuf)> {
        match self {
            Peers::Running { swarm, peers, into } => Ok((swarm, peers, into.clone())),
            Peers::Lazy {
                dir,
                config,
                peers,
                started,
            } => {
                if started.is_none() {
                    *started = Some(PackSwarm::start(dir, config.clone()).await?);
                }
                Ok((
                    started.as_ref().expect("started above"),
                    peers,
                    dir.join("downloads"),
                ))
            }
        }
    }

    /// Stops the session if this started one.
    pub(crate) async fn stop(self) {
        if let Peers::Lazy {
            started: Some(swarm),
            ..
        } = self
        {
            swarm.stop().await;
        }
    }
}

/// Downloads into `work` each pack of `manifest` that `wanted` accepts,
/// oldest first: from the first mirror that serves it intact, else from
/// peers. Returns each pack with the path of its verified file.
pub(crate) async fn download(
    manifest: &Manifest,
    wanted: impl Fn(InfoHash) -> bool,
    mirrors: &[(&str, MirrorSource)],
    peers: &mut Peers<'_>,
    work: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<Vec<(InfoHash, PathBuf)>> {
    let mut files = Vec::new();
    for (index, &pack) in manifest.packs().iter().enumerate() {
        if !wanted(pack) {
            continue;
        }
        let sha256 = manifest.packs_v2().map(|v2| v2[index]);
        let file = match from_mirrors(mirrors, pack, sha256, work).await {
            Ok(file) => file,
            Err(mut failures) => {
                let (swarm, peer_list, into) = peers.swarm().await?;
                let fetched = match swarm.fetch(pack, peer_list, &into, PACK_TIMEOUT).await {
                    // Peers check SHA-1 pieces; check SHA-256 here too.
                    Ok(file) => match verify_pack(&file, pack, sha256).await {
                        Ok(()) => Ok(file),
                        Err(error) => {
                            let _ = fs::remove_file(&file);
                            Err(error)
                        }
                    },
                    Err(error) => Err(error),
                };
                match fetched {
                    Ok(file) => file,
                    Err(error) => {
                        failures.push(format!("peers: {error}"));
                        bail!("no source has pack {pack}: {}", failures.join("; "));
                    }
                }
            }
        };
        writeln!(out, "Fetched pack {pack}.")?;
        files.push((pack, file));
    }
    Ok(files)
}

/// The pack from the first mirror that serves it intact, or why each failed.
async fn from_mirrors(
    mirrors: &[(&str, MirrorSource)],
    pack: InfoHash,
    sha256: Option<InfoHashV2>,
    work: &Path,
) -> Result<PathBuf, Vec<String>> {
    let mut failures = Vec::new();
    let dir = work.join("mirrors");
    for &(mirror, source) in mirrors {
        match fetch_from_mirror(mirror, source, pack, sha256, &dir, PACK_TIMEOUT).await {
            Ok(file) => return Ok(file),
            Err(error) => failures.push(error.to_string()),
        }
    }
    Err(failures)
}
