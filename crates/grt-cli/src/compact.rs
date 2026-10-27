use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, ensure};
use grt_git::Repo;

use crate::cli::{CompactArgs, SignArgs};
use crate::key;
use crate::sign::{self, Chain};

pub(crate) fn run(args: &CompactArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let previous = sign::load(&repo)?
        .context("nothing to compact: no signed manifest yet; run `git torrent sign`")?;
    let key = key::signing_key()?;
    ensure!(
        previous.public_key() == key.public_key(),
        "the signed manifest was signed by another key; only its publisher can compact it"
    );
    let name = String::from_utf8(previous.salt().to_vec())
        .context("the manifest's repository name is not UTF-8")?;
    let sign_args = SignArgs {
        upstream: previous.manifest().upstream().map(str::to_owned),
        name,
        mirrors: Vec::new(),
        updates: Vec::new(),
        deletes: Vec::new(),
        all: false,
        keep_refs: true,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let signed = runtime.block_on(sign::sign_next(
        &repo,
        &key,
        &sign_args,
        Chain::Compact,
        out,
    ))?;

    let packs_dir = repo.git_dir().join("torrent").join("packs");
    let mut old = Vec::new();
    for entry in fs::read_dir(&packs_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let in_chain = signed
            .manifest()
            .packs()
            .iter()
            .any(|pack| pack.to_string() == name);
        // `staging-*` directories belong to a sign in progress.
        if !in_chain && !name.starts_with("staging-") {
            old.push(entry.path());
        }
    }
    if old.is_empty() {
        return Ok(());
    }
    if args.prune {
        for dir in &old {
            fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        writeln!(out, "Pruned {} old pack(s).", old.len())?;
    } else {
        writeln!(
            out,
            "{} old pack(s) stay in .git/torrent/packs for peers still fetching them; \
             `git torrent compact --prune` deletes them.",
            old.len()
        )?;
    }
    Ok(())
}
