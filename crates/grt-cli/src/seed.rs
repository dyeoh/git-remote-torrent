use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use grt_core::{InfoHash, RemoteUrl, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig};

use crate::cli::SeedArgs;
use crate::{key, sign};

/// The pid file of a running seeder, under `.git/torrent/`.
pub(crate) const SEED_PID_FILE: &str = "seed.pid";

pub(crate) fn run(args: &SeedArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repos = repositories(args, cwd)?;
    if repos.len() > 1 {
        let sign = &args.sign;
        ensure!(
            sign.name.is_empty()
                && sign.mirrors.is_empty()
                && sign.upstream.is_none()
                && !sign.builds_on_previous(),
            "--name, --mirror, --upstream, --update and --delete apply to one repository; \
             set them with `git torrent sign` in each, then seed with several --repo"
        );
    }
    // Ask for any passphrase before the runtime and network start.
    let key = if repos.is_empty() {
        None
    } else {
        Some(key::signing_key()?)
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut served = Vec::new();
        for repo in repos {
            let mut sign_args = args.sign.clone();
            if args.repos.len() > 1 {
                // Each repository keeps publishing under its own name.
                sign_args.name = sign::load(&repo)?
                    .map(|previous| String::from_utf8_lossy(previous.salt()).into_owned())
                    .unwrap_or_default();
            }
            let key = key.as_ref().expect("loaded when there are repositories");
            let signed = sign::sign_next(&repo, key, &sign_args, sign::Chain::Extend, out).await?;
            served.push((repo, signed));
        }
        seed(served, args, out).await
    })
}

/// The repositories to serve: each `--repo`, else the current one. With only
/// `--republish` addresses, being outside a repository is fine.
fn repositories(args: &SeedArgs, cwd: &Path) -> anyhow::Result<Vec<Repo>> {
    if !args.repos.is_empty() {
        return args
            .repos
            .iter()
            .map(|path| {
                Repo::discover(&cwd.join(path))
                    .with_context(|| format!("{} is not a git repository", path.display()))
            })
            .collect();
    }
    match Repo::discover(cwd) {
        Ok(repo) => Ok(vec![repo]),
        Err(_) if !args.republish.is_empty() => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// One repository being seeded: its newest published manifest, and the pid
/// file that tells `git push` a seeder will publish what it signs.
struct Served {
    repo: Repo,
    current: SignedManifest,
    _pid_file: PidFile,
}

/// Seeds every pack of each repository's chain, publishes its manifest, and
/// keeps them alive until Ctrl-C, together with the `--republish` addresses.
/// Manifests signed meanwhile (by `git push` or `git torrent sign`) are
/// picked up, seeded and published too.
async fn seed(
    repos: Vec<(Repo, SignedManifest)>,
    args: &SeedArgs,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut config = SwarmConfig::default();
    config.listen = args.listen;
    config.dht = !args.no_peer_dht;
    config.upnp = args.upnp;
    let session = match repos.first() {
        Some((repo, _)) => repo.git_dir().join("torrent").join("session"),
        None => std::env::temp_dir().join(format!("git-torrent-seed-{}", std::process::id())),
    };
    let swarm = PackSwarm::start(&session, config).await?;
    let listen = swarm
        .listen_addr()
        .context("the torrent session is not listening")?;
    let dht = ManifestDht::connect(&args.dht_bootstrap)?;

    let mut seeded = HashSet::new();
    let mut served = Vec::new();
    for (repo, signed) in repos {
        seed_chain(&repo, &swarm, &signed, &mut seeded).await?;
        writeln!(
            out,
            "Seeding {} pack(s) of {} on {listen}.",
            signed.manifest().packs().len(),
            address(&signed)
        )?;
        publish(&dht, &signed).await?;
        writeln!(
            out,
            "Published manifest #{} to the DHT.",
            signed.seq().get()
        )?;
        out.flush()?;
        served.push(Served {
            _pid_file: PidFile::create(&repo)?,
            repo,
            current: signed,
        });
    }
    republish_others(&dht, &args.republish, out).await?;
    writeln!(out, "Press Ctrl-C to stop.")?;
    out.flush()?;

    // BEP 44 items expire after about two hours unless stored again.
    let mut republish = tokio::time::interval(Duration::from_secs(args.republish_secs));
    republish.tick().await;
    let mut watch = tokio::time::interval(WATCH_INTERVAL);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = republish.tick() => {
                for served in &served {
                    if let Err(error) = dht.publish(&served.current, None).await {
                        writeln!(out, "warning: republishing {} failed: {error}", address(&served.current))?;
                    }
                }
                republish_others(&dht, &args.republish, out).await?;
                out.flush()?;
            }
            _ = watch.tick() => {
                publish_newer(&mut served, &swarm, &dht, &mut seeded, out).await?;
                if served.is_empty() && args.republish.is_empty() {
                    bail!("no repository is left to serve");
                }
            }
        }
    }
    drop(served);
    swarm.stop().await;
    writeln!(out, "Stopped seeding.")?;
    Ok(())
}

/// Seeds and publishes each repository's newer manifest, if one was
/// signed. A repository whose publish loses the compare-and-swap race is
/// dropped from `served`.
async fn publish_newer(
    served: &mut Vec<Served>,
    swarm: &PackSwarm,
    dht: &ManifestDht,
    seeded: &mut HashSet<InfoHash>,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut lost = Vec::new();
    for (index, served) in served.iter_mut().enumerate() {
        let Some(next) = newer_manifest(&served.repo, &served.current) else {
            continue;
        };
        // Seed first, so the manifest never points at packs no one has.
        seed_chain(&served.repo, swarm, &next, seeded).await?;
        match dht.publish(&next, Some(served.current.seq())).await {
            Ok(()) => writeln!(
                out,
                "Published manifest #{} of {} ({} pack(s)).",
                next.seq().get(),
                address(&next),
                next.manifest().packs().len()
            )?,
            // Republishing the loser would fight the other machine.
            Err(grt_swarm::Error::Conflict(reason)) => {
                writeln!(
                    out,
                    "error: manifest #{} of {} was not published: another machine \
                     published to this address meanwhile ({reason}). Only one machine \
                     can publish an address; no longer serving it.",
                    next.seq().get(),
                    address(&next)
                )?;
                lost.push(index);
                continue;
            }
            Err(error) => writeln!(
                out,
                "warning: could not publish manifest #{}: {error}",
                next.seq().get()
            )?,
        }
        // Even if publishing failed, republishing retries the newest.
        served.current = next;
    }
    for index in lost.into_iter().rev() {
        served.remove(index);
    }
    out.flush()?;
    Ok(())
}

/// Stores each address's current manifest again; reports addresses with
/// nothing to republish without stopping.
async fn republish_others(
    dht: &ManifestDht,
    addresses: &[RemoteUrl],
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    for address in addresses {
        let RemoteUrl::Swarm { key, salt } = address else {
            bail!("{address} is not a torrent://<key> address");
        };
        match dht.resolve(key, salt).await {
            Ok(Some(signed)) => match dht.publish(&signed, None).await {
                Ok(()) => writeln!(
                    out,
                    "Republished manifest #{} of {address}.",
                    signed.seq().get()
                )?,
                Err(error) => writeln!(out, "warning: republishing {address} failed: {error}")?,
            },
            Ok(None) => writeln!(out, "warning: no manifest found for {address}")?,
            Err(error) => writeln!(out, "warning: resolving {address} failed: {error}")?,
        }
    }
    Ok(())
}

fn address(signed: &SignedManifest) -> RemoteUrl {
    RemoteUrl::Swarm {
        key: signed.public_key(),
        salt: signed.salt().to_vec(),
    }
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
