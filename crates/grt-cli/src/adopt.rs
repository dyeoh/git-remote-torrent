use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use grt_core::{InfoHash, InfoHashV2, RemoteUrl, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig, fetch_from_mirror, verify_pack};

use crate::cli::AdoptArgs;
use crate::{key, sign};

/// Deadline for each pack download.
const PACK_TIMEOUT: Duration = Duration::from_secs(120);

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
    let signed = runtime
        .block_on(async {
            let dht = ManifestDht::connect(&args.dht_bootstrap)?;
            dht.resolve(&key.public_key(), args.name.as_bytes()).await
        })?
        .with_context(|| {
            format!("nothing is published at {address}, which is your key's address; run `git torrent sign` to start one")
        })?;
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
    let mut mirrors: Vec<&str> = manifest.mirrors().iter().map(String::as_str).collect();
    mirrors.extend(args.mirrors.iter().map(String::as_str));

    let mut swarm = None;
    let mut files = Vec::new();
    for (index, &pack) in manifest.packs().iter().enumerate() {
        let sha256 = manifest.packs_v2().map(|v2| v2[index]);
        let file = match from_mirrors(&mirrors, pack, sha256, work).await {
            Ok(file) => file,
            Err(mut failures) => {
                if swarm.is_none() {
                    let mut config = SwarmConfig::default();
                    config.dht = args.dht_bootstrap.is_empty();
                    swarm = Some(PackSwarm::start(&work.join("session"), config).await?);
                }
                let swarm = swarm.as_ref().expect("started above");
                let fetched = match swarm
                    .fetch(pack, &args.peers, &work.join("peers"), PACK_TIMEOUT)
                    .await
                {
                    Ok(file) => verify_pack(&file, pack, sha256).await.map(|()| file),
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
    if let Some(swarm) = swarm {
        swarm.stop().await;
    }

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

/// The pack from the first mirror that serves it intact, or why each failed.
async fn from_mirrors(
    mirrors: &[&str],
    pack: InfoHash,
    sha256: Option<InfoHashV2>,
    work: &Path,
) -> Result<PathBuf, Vec<String>> {
    let mut failures = Vec::new();
    for mirror in mirrors {
        match fetch_from_mirror(mirror, pack, sha256, &work.join("mirrors"), PACK_TIMEOUT).await {
            Ok(file) => return Ok(file),
            Err(error) => failures.push(error.to_string()),
        }
    }
    Err(failures)
}
