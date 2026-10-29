use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail, ensure};
use grt_core::{RemoteUrl, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, MirrorSource, SwarmConfig};

use crate::cli::AdoptArgs;
use crate::packs::{self, Peers};
use crate::{key, sign};

pub(crate) fn run(args: &AdoptArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let key = key::signing_key()?;
    let address = RemoteUrl::Swarm {
        key: key.public_key(),
        salt: args.name.as_bytes().to_vec(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let from_dht = runtime.block_on(async {
        let dht = ManifestDht::connect(&args.dht_bootstrap)?;
        dht.resolve(&key.public_key(), args.name.as_bytes()).await
    })?;
    let from_file = args.manifest.as_deref().map(sign::read).transpose()?;
    if let Some(copy) = &from_file
        && (copy.public_key() != key.public_key() || copy.salt() != args.name.as_bytes())
    {
        bail!(
            "{} is a manifest of {}, not of {address}",
            args.manifest.as_deref().unwrap_or(Path::new("")).display(),
            sign::address(copy)
        );
    }
    let signed = match (from_dht, from_file) {
        (Some(published), Some(copy))
            if copy.seq() == published.seq() && copy.value() != published.value() =>
        {
            bail!(
                "the DHT has a different manifest #{} than the copy; another machine is \
                 publishing with this key and name",
                published.seq().get()
            )
        }
        // An expired or not yet republished address: the copy is newer.
        (Some(published), Some(copy)) if copy.seq() > published.seq() => copy,
        (Some(published), _) => published,
        (None, Some(copy)) => copy,
        (None, None) if args.allow_new => {
            writeln!(
                out,
                "Nothing is published at {address} yet; `git torrent sign` starts its chain."
            )?;
            return Ok(());
        }
        (None, None) => bail!(
            "nothing is published at {address}, which is your key's address; run \
             `git torrent sign` to start one"
        ),
    };
    if let Some(local) = sign::load(&repo)?
        && local.public_key() == signed.public_key()
        && local.salt() == signed.salt()
        && local.seq() >= signed.seq()
    {
        writeln!(
            out,
            "Already up to date: this repository has manifest #{} of {address}.",
            local.seq().get()
        )?;
        return Ok(());
    }

    let work = repo.git_dir().join("torrent").join("adopt");
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    let result = runtime.block_on(adopt(&repo, &signed, args, &work, out));
    // Downloads are moved into place or discarded; nothing half-done stays.
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    result?;
    sign::save(&repo, &signed)?;
    writeln!(
        out,
        "Adopted manifest #{} of {address} with {} pack(s); `git torrent sign` and \
         `git push` continue its chain.",
        signed.seq().get(),
        signed.manifest().packs().len()
    )?;
    Ok(())
}

/// Downloads and verifies the chain, proves it complete in a scratch
/// repository, then stores the packs and indexes them into `repo`.
async fn adopt(
    repo: &Repo,
    signed: &SignedManifest,
    args: &AdoptArgs,
    work: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let manifest = signed.manifest();
    // Your own key signed the manifest, so its mirrors are your choice.
    let mirrors: Vec<(&str, MirrorSource)> = manifest
        .mirrors()
        .iter()
        .chain(&args.mirrors)
        .map(|mirror| (mirror.as_str(), MirrorSource::User))
        .collect();
    let mut config = SwarmConfig::default();
    config.dht = args.dht_bootstrap.is_empty();
    let mut peers = Peers::Lazy {
        dir: work.join("session"),
        config,
        peers: &args.peers,
        started: None,
    };
    let files = packs::download(manifest, |_| true, &mirrors, &mut peers, work, out).await;
    peers.stop().await;
    let files = files?;

    // The chain must rebuild every published ref by itself, oldest first.
    let scratch_dir = work.join("check.git");
    let status = Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&scratch_dir)
        .status()?;
    ensure!(status.success(), "git init failed");
    let scratch = Repo::discover(&scratch_dir)?;
    for (pack, file) in &files {
        scratch
            .index_pack(file)
            .with_context(|| format!("pack {pack} does not apply after the ones before it"))?;
    }
    for (name, id) in manifest.refs() {
        if !scratch.contains(id)? {
            bail!("the pack chain does not contain {name} ({id}); not adopting it");
        }
    }

    for (pack, file) in &files {
        let stored = sign::pack_path(repo, *pack);
        if !stored.exists() {
            let dir = stored.parent().expect("packs live in a directory");
            fs::create_dir_all(dir)?;
            fs::rename(file, &stored)?;
        }
        // So the next pack holds only what is new, even in a fresh checkout.
        repo.index_pack(&stored)?;
    }
    Ok(())
}
