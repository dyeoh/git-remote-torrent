use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use grt_core::SignedManifest;
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig};

use crate::cli::SeedArgs;
use crate::{key, sign};

pub(crate) fn run(args: &SeedArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    // Ask for any passphrase before the runtime and network start.
    let key = key::signing_key()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let signed = sign::sign_next(&repo, &key, &args.sign, out).await?;
        seed(&repo, &signed, args, out).await
    })
}

/// Seeds every pack of `signed`'s chain, publishes it, and keeps both alive
/// until Ctrl-C.
async fn seed(
    repo: &Repo,
    signed: &SignedManifest,
    args: &SeedArgs,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut config = SwarmConfig::default();
    config.listen = args.listen;
    config.dht = !args.no_peer_dht;
    let swarm = PackSwarm::start(&repo.git_dir().join("torrent").join("session"), config).await?;
    for &pack in signed.manifest().packs() {
        let path = sign::pack_path(repo, pack);
        ensure!(
            path.exists(),
            "pack {pack} of the chain is missing ({}); it was signed on another machine. \
             Delete .git/torrent/manifest to start a new chain.",
            path.display()
        );
        let seeded = swarm.seed(&path).await?;
        ensure!(
            seeded == pack,
            "{} does not match its infohash {pack}",
            path.display()
        );
    }
    let listen = swarm
        .listen_addr()
        .context("the torrent session is not listening")?;
    writeln!(
        out,
        "Seeding {} pack(s) on {listen}.",
        signed.manifest().packs().len()
    )?;
    out.flush()?;

    let dht = if args.dht_bootstrap.is_empty() {
        ManifestDht::public()?
    } else {
        ManifestDht::with_bootstrap(&args.dht_bootstrap, bind_address(&args.dht_bootstrap))?
    };
    publish(&dht, signed).await?;
    writeln!(
        out,
        "Published manifest #{} to the DHT. Press Ctrl-C to stop.",
        signed.seq().get()
    )?;
    out.flush()?;

    // BEP 44 items expire after about two hours unless stored again.
    let mut republish = tokio::time::interval(Duration::from_secs(args.republish_secs));
    republish.tick().await;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = republish.tick() => {
                if let Err(error) = dht.publish(signed, None).await {
                    writeln!(out, "warning: republishing failed: {error}")?;
                    out.flush()?;
                }
            }
        }
    }
    swarm.stop().await;
    writeln!(out, "Stopped seeding.")?;
    Ok(())
}

/// Publishes `signed` unless a different manifest at least as new is already
/// on the DHT, which means another machine published under this address.
async fn publish(dht: &ManifestDht, signed: &SignedManifest) -> anyhow::Result<()> {
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

/// Private networks of loopback nodes need a loopback socket.
fn bind_address(bootstrap: &[String]) -> Ipv4Addr {
    let all_loopback = bootstrap.iter().all(|node| {
        node.parse::<SocketAddr>()
            .is_ok_and(|addr| addr.ip().is_loopback())
    });
    if all_loopback {
        Ipv4Addr::LOCALHOST
    } else {
        Ipv4Addr::UNSPECIFIED
    }
}
