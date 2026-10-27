use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};
use grt_core::SignedManifest;
use grt_git::Repo;
use grt_swarm::{ManifestDht, MirrorSource, fetch_from_mirror};

use crate::cli::{PublishArgs, VerifyMirrorArgs};
use crate::{seed, sign};

/// Deadline for each pack downloaded from a mirror.
const PACK_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn run(args: &PublishArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let signed = signed(&repo)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let dht = ManifestDht::connect(&args.dht_bootstrap)?;
        // Refuses if another machine published a newer or different manifest.
        seed::publish(&dht, &signed).await
    })?;
    writeln!(
        out,
        "Published manifest #{} to the DHT.",
        signed.seq().get()
    )?;
    if signed.manifest().mirrors().is_empty() {
        writeln!(
            out,
            "warning: the manifest lists no mirror, so its packs are only available while \
             `git torrent seed` runs somewhere"
        )?;
    }
    writeln!(
        out,
        "It expires from the DHT in about two hours unless republished \
         (`git torrent republish`)."
    )?;
    Ok(())
}

pub(crate) fn verify_mirror(
    args: &VerifyMirrorArgs,
    cwd: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let signed = signed(&repo)?;
    let manifest = signed.manifest();
    let mirrors = if args.mirrors.is_empty() {
        manifest.mirrors().to_vec()
    } else {
        args.mirrors.clone()
    };
    if mirrors.is_empty() {
        bail!("the manifest lists no mirror; name one to check, or sign with --mirror");
    }
    let work = repo.git_dir().join("torrent").join("verify-mirror");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut broken = Vec::new();
    for mirror in &mirrors {
        let mut failures = Vec::new();
        for (index, &pack) in manifest.packs().iter().enumerate() {
            let sha256 = manifest.packs_v2().map(|v2| v2[index]);
            // The manifest is your own, so its mirrors are your choice.
            let fetched = fetch_from_mirror(
                mirror,
                MirrorSource::User,
                pack,
                sha256,
                &work,
                PACK_TIMEOUT,
            );
            if let Err(error) = runtime.block_on(fetched) {
                failures.push(error.to_string());
            }
        }
        if work.exists() {
            fs::remove_dir_all(&work)?;
        }
        if failures.is_empty() {
            writeln!(
                out,
                "{mirror}: all {} pack(s) of manifest #{} verified.",
                manifest.packs().len(),
                signed.seq().get()
            )?;
        } else {
            for failure in &failures {
                writeln!(out, "{mirror}: {failure}")?;
            }
            broken.push(mirror.as_str());
        }
    }
    if !broken.is_empty() {
        bail!(
            "{} cannot serve this manifest's packs; upload them (`git torrent export`) \
             before publishing",
            broken.join(", ")
        );
    }
    Ok(())
}

fn signed(repo: &Repo) -> anyhow::Result<SignedManifest> {
    sign::load(repo)?.context("no signed manifest yet; run `git torrent sign` first")
}
