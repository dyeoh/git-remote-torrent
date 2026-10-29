use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use grt_core::{InfoHash, PublicKey, RemoteUrl, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, MirrorSource, PackSwarm, SwarmConfig};

use crate::cli::SeedArgs;
use crate::packs::{self, Peers};
use crate::{key, sign};

/// The pid file of a running seeder, under `.git/torrent/`.
pub(crate) const SEED_PID_FILE: &str = "seed.pid";

pub(crate) fn run(args: &SeedArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    if args.service.install || args.service.uninstall || args.service.print_service {
        return crate::service::run(args, cwd, out);
    }
    let repos = repositories(args, cwd)?;
    let follows = follow_targets(args, cwd)?;
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
        seed(served, follows, args, out).await
    })
}

/// The repositories to sign and serve: each `--repo`, else the current one.
/// With only `--follow` remotes or `--republish` addresses, there are none.
pub(crate) fn repositories(args: &SeedArgs, cwd: &Path) -> anyhow::Result<Vec<Repo>> {
    if args.repos.is_empty() && !args.follow.is_empty() {
        return Ok(Vec::new());
    }
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

/// A `--follow` remote: the repository it belongs to, its address, and the
/// repository's `torrent.peer`, `torrent.mirror` and
/// `torrent.allowPrivateMirrors` settings, as the remote helper uses them.
pub(crate) struct FollowTarget {
    repo: Repo,
    remote: String,
    key: PublicKey,
    salt: Vec<u8>,
    sources: Sources,
}

/// Where a follower looks for packs besides the DHT.
pub(crate) struct Sources {
    peers: Vec<SocketAddr>,
    mirrors: Vec<String>,
    allow_private_mirrors: bool,
}

impl Sources {
    fn from_config(repo: &Repo) -> anyhow::Result<Self> {
        let peers = repo
            .config_all("torrent.peer")?
            .iter()
            .map(|peer| {
                peer.parse()
                    .with_context(|| format!("torrent.peer {peer:?} is not host:port"))
            })
            .collect::<anyhow::Result<_>>()?;
        let allow_private_mirrors = match repo.config("torrent.allowPrivateMirrors")?.as_deref() {
            None | Some("false" | "no" | "off" | "0") => false,
            Some("true" | "yes" | "on" | "1") => true,
            Some(other) => bail!("torrent.allowPrivateMirrors must be a boolean, not {other:?}"),
        };
        Ok(Sources {
            peers,
            mirrors: repo.config_all("torrent.mirror")?,
            allow_private_mirrors,
        })
    }
}

/// The `--follow` remotes, in the first `--repo` or the current repository.
/// Each must be a `torrent://` remote or have a `torrentAddress`.
pub(crate) fn follow_targets(args: &SeedArgs, cwd: &Path) -> anyhow::Result<Vec<FollowTarget>> {
    if args.follow.is_empty() {
        return Ok(Vec::new());
    }
    let dir = args
        .repos
        .first()
        .map_or(cwd.to_path_buf(), |repo| cwd.join(repo));
    args.follow
        .iter()
        .map(|remote| {
            let repo = Repo::discover(&dir)
                .with_context(|| format!("{} is not a git repository", dir.display()))?;
            let url = repo.config(&format!("remote.{remote}.url"))?;
            let address = match url.as_deref() {
                Some(url) if url.starts_with("torrent://") => url.to_owned(),
                Some(_) => repo
                    .config(&format!("remote.{remote}.torrentAddress"))?
                    .with_context(|| {
                        format!(
                            "remote {remote} has no torrent address \
                             (remote.{remote}.torrentAddress) to follow"
                        )
                    })?,
                None => bail!("there is no remote named {remote}"),
            };
            let RemoteUrl::Swarm { key, salt } = address.parse()? else {
                bail!("{address} is not a torrent:// address");
            };
            Ok(FollowTarget {
                sources: Sources::from_config(&repo)?,
                repo,
                remote: remote.clone(),
                key,
                salt,
            })
        })
        .collect()
}

/// A followed address: the newest manifest seen for it, whose packs are
/// seeded and which is republished.
struct Followed {
    repo: Repo,
    address: RemoteUrl,
    sources: Sources,
    current: SignedManifest,
}

/// One repository being seeded: its newest published manifest, and the pid
/// file that tells `git push` a seeder will publish what it signs.
struct Served {
    repo: Repo,
    current: SignedManifest,
    _pid_file: PidFile,
}

/// Seeds every pack of each repository's chain, publishes its manifest, and
/// keeps them alive until Ctrl-C, together with the `--follow` and
/// `--republish` addresses. Manifests signed meanwhile (by `git push` or
/// `git torrent sign`) are picked up, seeded and published too, as are newer
/// manifests of followed addresses.
async fn seed(
    repos: Vec<(Repo, SignedManifest)>,
    follows: Vec<FollowTarget>,
    args: &SeedArgs,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut config = SwarmConfig::default();
    config.listen = args.listen;
    config.dht = !args.no_peer_dht;
    config.local_discovery = !args.no_local_discovery;
    config.upnp = args.upnp;
    let session = match (repos.first(), follows.first()) {
        (Some((repo, _)), _) | (None, Some(FollowTarget { repo, .. })) => {
            repo.git_dir().join("torrent").join("session")
        }
        (None, None) => {
            std::env::temp_dir().join(format!("git-torrent-seed-{}", std::process::id()))
        }
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
            sign::address(&signed)
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
    let mut followed = Vec::new();
    for target in follows {
        followed.push(follow(target, &swarm, &dht, &mut seeded, out).await?);
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
                        writeln!(out, "warning: republishing {} failed: {error}", sign::address(&served.current))?;
                    }
                }
                follow_newer(&mut followed, &swarm, &dht, &mut seeded, out).await?;
                republish_others(&dht, &args.republish, out).await?;
                out.flush()?;
            }
            _ = watch.tick() => {
                publish_newer(&mut served, &swarm, &dht, &mut seeded, out).await?;
                if served.is_empty() && followed.is_empty() && args.republish.is_empty() {
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
                sign::address(&next),
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
                    sign::address(&next)
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

/// Starts following `target`: takes the newer of the manifest its remote last
/// fetched and the one on the DHT, stores and seeds its packs, and publishes
/// it again.
async fn follow(
    target: FollowTarget,
    swarm: &PackSwarm,
    dht: &ManifestDht,
    seeded: &mut HashSet<InfoHash>,
    out: &mut dyn Write,
) -> anyhow::Result<Followed> {
    let FollowTarget {
        repo,
        remote,
        key,
        salt,
        sources,
    } = target;
    let address = RemoteUrl::Swarm {
        key,
        salt: salt.clone(),
    };
    let cache = grt_swarm::fetched_manifest_path(&repo.git_dir().join("torrent"), &key, &salt);
    // Decoding verifies the signature; the key and name must be the remote's.
    let fetched = fs::read(&cache)
        .ok()
        .and_then(|bytes| SignedManifest::decode(&bytes).ok())
        .filter(|signed| signed.public_key() == key && signed.salt() == salt);
    let published = match dht.resolve(&key, &salt).await {
        Ok(published) => published,
        Err(error) => {
            writeln!(out, "warning: resolving {address} failed: {error}")?;
            None
        }
    };
    let current = match (fetched, published) {
        (Some(fetched), Some(published)) if published.seq() > fetched.seq() => published,
        (Some(fetched), _) => fetched,
        (None, Some(published)) => published,
        (None, None) => bail!(
            "nothing is published at {address} ({remote}), and {remote} has not been fetched \
             from it"
        ),
    };
    stock(&repo, &current, &sources, swarm, seeded, out).await?;
    writeln!(
        out,
        "Following {remote}: seeding {} pack(s) of {address} manifest #{} on {}.",
        current.manifest().packs().len(),
        current.seq().get(),
        swarm
            .listen_addr()
            .context("the torrent session is not listening")?
    )?;
    if let Err(error) = dht.publish(&current, None).await {
        writeln!(out, "warning: republishing {address} failed: {error}")?;
    }
    Ok(Followed {
        repo,
        address,
        sources,
        current,
    })
}

/// Picks up a newer manifest for each followed address: its new packs are
/// stored and seeded before it is republished. Failures are reported and
/// retried on the next tick.
async fn follow_newer(
    followed: &mut [Followed],
    swarm: &PackSwarm,
    dht: &ManifestDht,
    seeded: &mut HashSet<InfoHash>,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    for followed in followed {
        let RemoteUrl::Swarm { key, salt } = &followed.address else {
            unreachable!("followed addresses are torrent:// addresses");
        };
        match dht.resolve(key, salt).await {
            Ok(Some(next)) if next.seq() > followed.current.seq() => {
                match stock(&followed.repo, &next, &followed.sources, swarm, seeded, out).await {
                    Ok(()) => {
                        writeln!(
                            out,
                            "Now seeding manifest #{} of {} ({} pack(s)).",
                            next.seq().get(),
                            followed.address,
                            next.manifest().packs().len()
                        )?;
                        followed.current = next;
                    }
                    Err(error) => writeln!(
                        out,
                        "warning: fetching the packs of manifest #{} of {} failed: {error:#}",
                        next.seq().get(),
                        followed.address
                    )?,
                }
            }
            Ok(_) => {}
            Err(error) => writeln!(
                out,
                "warning: resolving {} failed: {error}",
                followed.address
            )?,
        }
        if let Err(error) = dht.publish(&followed.current, None).await {
            writeln!(
                out,
                "warning: republishing {} failed: {error}",
                followed.address
            )?;
        }
    }
    Ok(())
}

/// Stores every pack of `signed` that `repo` lacks, from the manifest's
/// mirrors or from peers, then seeds the chain.
async fn stock(
    repo: &Repo,
    signed: &SignedManifest,
    sources: &Sources,
    swarm: &PackSwarm,
    seeded: &mut HashSet<InfoHash>,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let torrent_dir = repo.git_dir().join("torrent");
    // The publisher chose the manifest's mirrors, so unless the repository
    // allows otherwise they may only reach public hosts (threat model M1).
    let manifest_source = if sources.allow_private_mirrors {
        MirrorSource::User
    } else {
        MirrorSource::Manifest
    };
    let mirrors: Vec<(&str, MirrorSource)> = signed
        .manifest()
        .mirrors()
        .iter()
        .map(|mirror| (mirror.as_str(), manifest_source))
        .chain(
            sources
                .mirrors
                .iter()
                .map(|mirror| (mirror.as_str(), MirrorSource::User)),
        )
        .collect();
    let work = torrent_dir.join(format!("follow-{}", std::process::id()));
    // Peer downloads land in the store itself, where the session seeds them.
    let mut peers = Peers::Running {
        swarm,
        peers: &sources.peers,
        into: grt_swarm::packs_dir(&torrent_dir),
    };
    let downloaded = packs::download(
        signed.manifest(),
        |pack| !grt_swarm::stored_pack_path(&torrent_dir, pack).exists(),
        &mirrors,
        &mut peers,
        &work,
        out,
    )
    .await;
    let stored = downloaded.and_then(|files| {
        for (pack, file) in files {
            let stored = grt_swarm::stored_pack_path(&torrent_dir, pack);
            if file != stored {
                fs::create_dir_all(stored.parent().expect("packs live in a directory"))?;
                fs::rename(&file, &stored)?;
            }
        }
        Ok(())
    });
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    stored?;
    seed_chain(repo, swarm, signed, seeded).await
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
