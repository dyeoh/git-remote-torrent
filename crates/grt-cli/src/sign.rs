use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use grt_core::{Manifest, RemoteUrl, Seq, SignedManifest};
use grt_git::Repo;

use crate::cli::SignArgs;
use crate::key;

/// Refs included in a manifest.
pub(crate) const PUBLISHED_REFS: &[&str] = &["refs/heads", "refs/tags"];

pub(crate) fn run(args: &SignArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let key = key::signing_key()?;

    let mut manifest = Manifest::new();
    for (name, id) in repo.refs(PUBLISHED_REFS)? {
        manifest.insert_ref(name, id)?;
    }
    if let Some(head) = repo.head_target()? {
        if manifest.refs().contains_key(&head) {
            manifest.set_head(Some(head))?;
        }
    }
    manifest.set_upstream(match &args.upstream {
        Some(upstream) => Some(upstream.clone()),
        None => origin_upstream(&repo)?,
    });

    let seq = match load(&repo)? {
        Some(previous)
            if previous.public_key() == key.public_key()
                && previous.salt() == args.name.as_bytes() =>
        {
            previous.seq().next()?
        }
        _ => Seq::new(1),
    };
    let signed = key
        .sign(&manifest, seq, args.name.as_bytes())
        .map_err(|error| match error {
            grt_core::Error::TooLarge { .. } => anyhow::anyhow!(
                "{error}: too many refs for a version 1 manifest (about 20 fit); \
             delete stale branches or tags and try again"
            ),
            other => other.into(),
        })?;
    save(&repo, &signed)?;

    writeln!(
        out,
        "Signed manifest #{} with {} ref(s){}.",
        signed.seq().get(),
        manifest.refs().len(),
        manifest
            .head()
            .map(|head| format!(", HEAD -> {head}"))
            .unwrap_or_default()
    )?;
    let address = RemoteUrl::Swarm {
        key: signed.public_key(),
        salt: signed.salt().to_vec(),
    };
    writeln!(out, "Address: {address}")?;
    Ok(())
}

/// The canonical upstream from `remote.origin.url`, unwrapping `torrent::`.
fn origin_upstream(repo: &Repo) -> anyhow::Result<Option<String>> {
    let Some(url) = repo.config("remote.origin.url")? else {
        return Ok(None);
    };
    if let Some(upstream) = url.strip_prefix("torrent::") {
        return Ok(Some(upstream.to_owned()));
    }
    if url.starts_with("torrent://") {
        return Ok(None);
    }
    Ok(Some(url))
}

/// Loads and re-verifies the cached signed manifest, if there is one.
pub(crate) fn load(repo: &Repo) -> anyhow::Result<Option<SignedManifest>> {
    let path = path(repo);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    match SignedManifest::decode(&bytes) {
        Ok(signed) => Ok(Some(signed)),
        Err(error) => bail!(
            "{} failed verification ({error}); it may have been tampered with",
            path.display()
        ),
    }
}

fn save(repo: &Repo, signed: &SignedManifest) -> anyhow::Result<()> {
    let path = path(repo);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, signed.encode()).with_context(|| format!("writing {}", path.display()))
}

fn path(repo: &Repo) -> PathBuf {
    repo.git_dir().join("torrent").join("manifest")
}
