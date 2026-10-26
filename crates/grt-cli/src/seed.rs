use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use grt_core::{InfoHash, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig};

use crate::cli::SeedArgs;
use crate::{key, sign};

/// The pid file of a running seeder, under `.git/torrent/`.
pub(crate) const SEED_PID_FILE: &str = "seed.pid";

pub(crate) fn run(args: &SeedArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    // Ask for any passphrase before the runtime and network start.
    let key = key::signing_key()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let signed = sign::sign_next(&repo, &key, &args.sign, sign::Chain::Extend, out).await?;
        seed(&repo, &signed, args, out).await
    })
}

/// Seeds every pack of `signed`'s chain, publishes it, and keeps both alive
/// until Ctrl-C. Manifests signed meanwhile (by `git push` or
/// `git torrent sign`) are picked up, seeded and published too.
async fn seed(
    repo: &Repo,
    signed: &SignedManifest,
    args: &SeedArgs,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut config = SwarmConfig::default();
    config.listen = args.listen;
    config.dht = !args.no_peer_dht;
    config.upnp = args.upnp;
    let swarm = PackSwarm::start(&repo.git_dir().join("torrent").join("session"), config).await?;
    let mut seeded = HashSet::new();
    seed_chain(repo, &swarm, signed, &mut seeded).await?;
    let listen = swarm
        .listen_addr()
        .context("the torrent session is not listening")?;
    writeln!(
        out,
        "Seeding {} pack(s) on {listen}.",
        signed.manifest().packs().len()
    )?;
    out.flush()?;

    let dht = ManifestDht::connect(&args.dht_bootstrap)?;
    publish(&dht, signed).await?;
    writeln!(
        out,
        "Published manifest #{} to the DHT. Press Ctrl-C to stop.",
        signed.seq().get()
    )?;
    out.flush()?;

    let pid_file = PidFile::create(repo)?;
    let mut current = signed.clone();
    // BEP 44 items expire after about two hours unless stored again.
    let mut republish = tokio::time::interval(Duration::from_secs(args.republish_secs));
    republish.tick().await;
    let mut watch = tokio::time::interval(WATCH_INTERVAL);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = republish.tick() => {
                if let Err(error) = dht.publish(&current, None).await {
                    writeln!(out, "warning: republishing failed: {error}")?;
                    out.flush()?;
                }
            }
            _ = watch.tick() => {
                let Some(next) = newer_manifest(repo, &current) else {
                    continue;
                };
                // Seed first, so the manifest never points at packs no one has.
                seed_chain(repo, &swarm, &next, &mut seeded).await?;
                match dht.publish(&next, Some(current.seq())).await {
                    Ok(()) => writeln!(
                        out,
                        "Published manifest #{} ({} pack(s)).",
                        next.seq().get(),
                        next.manifest().packs().len()
                    )?,
                    // Republishing the loser would fight the other machine.
                    Err(grt_swarm::Error::Conflict(reason)) => bail!(
                        "manifest #{} was not published: another machine published to this \
                         address meanwhile ({reason}). Only one machine can publish an address.",
                        next.seq().get()
                    ),
                    Err(error) => writeln!(
                        out,
                        "warning: could not publish manifest #{}: {error}",
                        next.seq().get()
                    )?,
                }
                out.flush()?;
                // Even if publishing failed, republishing retries the newest.
                current = next;
            }
        }
    }
    drop(pid_file);
    swarm.stop().await;
    writeln!(out, "Stopped seeding.")?;
    Ok(())
}

/// How often the cached manifest is checked for one signed by `git push`.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Seeds the packs of `signed`'s chain that are not in `seeded` yet.
async fn seed_chain(
    repo: &Repo,
    swarm: &PackSwarm,
    signed: &SignedManifest,
    seeded: &mut HashSet<InfoHash>,
) -> anyhow::Result<()> {
    for &pack in signed.manifest().packs() {
        if seeded.contains(&pack) {
            continue;
        }
        let path = sign::pack_path(repo, pack);
        ensure!(
            path.exists(),
            "pack {pack} of the chain is missing ({}); it was signed on another machine. \
             Delete .git/torrent/manifest to start a new chain.",
            path.display()
        );
        let found = swarm.seed(&path).await?;
        ensure!(
            found == pack,
            "{} does not match its infohash {pack}",
            path.display()
        );
        seeded.insert(pack);
    }
    Ok(())
}

/// The cached manifest, if it is a later one for the same address.
fn newer_manifest(repo: &Repo, current: &SignedManifest) -> Option<SignedManifest> {
    let next = sign::load(repo).ok().flatten()?;
    (next.public_key() == current.public_key()
        && next.salt() == current.salt()
        && next.seq() > current.seq())
    .then_some(next)
}

/// `.git/torrent/seed.pid` while a seeder runs, so `git push` can tell
/// whether its new manifest will be published.
struct PidFile(PathBuf);

impl PidFile {
    fn create(repo: &Repo) -> anyhow::Result<Self> {
        let path = repo.git_dir().join("torrent").join(SEED_PID_FILE);
        fs::write(&path, std::process::id().to_string())?;
        Ok(PidFile(path))
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Publishes `signed` unless a different manifest at least as new is already
/// on the DHT, which means another machine published under this address.
pub(crate) async fn publish(dht: &ManifestDht, signed: &SignedManifest) -> anyhow::Result<()> {
    let current = dht.resolve(&signed.public_key(), signed.salt()).await?;
    let cas = match current {
        Some(current) if current.seq() > signed.seq() => bail!(
            "manifest #{} is already published at this address, newer than #{}. \
             Another machine is publishing with this key and name.",
            current.seq().get(),
            signed.seq().get()
        ),
        Some(current) if current.seq() == signed.seq() && current.value() != signed.value() => {
            bail!(
                "a different manifest #{} is already published at this address. \
             Another machine is publishing with this key and name.",
                current.seq().get()
            )
        }
        Some(current) if current.seq() < signed.seq() => Some(current.seq()),
        _ => None,
    };
    dht.publish(signed, cas).await?;
    Ok(())
}
