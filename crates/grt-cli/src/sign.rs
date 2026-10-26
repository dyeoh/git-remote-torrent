use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use grt_core::{InfoHash, Manifest, ObjectId, PublisherKey, RemoteUrl, Seq, SignedManifest};
use grt_git::Repo;

use crate::cli::SignArgs;
use crate::key;

/// Refs included in a manifest.
pub(crate) const PUBLISHED_REFS: &[&str] = &["refs/heads", "refs/tags"];

/// Every pack is stored as `.git/torrent/packs/<infohash>/pack`. The torrent
/// is named `pack`, so its infohash depends only on the pack's content.
const PACK_FILE: &str = "pack";

pub(crate) fn run(args: &SignArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let key = key::signing_key()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(sign_next(&repo, &key, args, out))?;
    Ok(())
}

/// Signs the next manifest for the local refs and caches it.
///
/// If the refs reach objects the previous manifest's packs do not hold, the
/// chain grows by one pack first (ADR-0004), so every signed manifest's packs
/// contain its refs.
pub(crate) async fn sign_next(
    repo: &Repo,
    key: &PublisherKey,
    args: &SignArgs,
    out: &mut dyn Write,
) -> anyhow::Result<SignedManifest> {
    // The chain continues only for the same key and name; anything else starts over.
    let previous = load(repo)?.filter(|previous| {
        previous.public_key() == key.public_key() && previous.salt() == args.name.as_bytes()
    });

    let mut manifest = refs_manifest(repo, args, previous.as_ref())?;
    let mut packs = previous
        .as_ref()
        .map(|previous| previous.manifest().packs().to_vec())
        .unwrap_or_default();
    let exclude: Vec<ObjectId> = previous
        .as_ref()
        .map(|previous| previous.manifest().refs().values().copied().collect())
        .unwrap_or_default();
    if let Some((pack, objects)) = write_next_pack(repo, &manifest, &exclude).await? {
        writeln!(out, "Added pack {pack} ({objects} objects) to the chain.")?;
        packs.push(pack);
    }
    for pack in packs {
        manifest.push_pack(pack);
    }

    let seq = match &previous {
        Some(previous) => previous.seq().next()?,
        None => Seq::new(1),
    };
    let signed = key
        .sign(&manifest, seq, args.name.as_bytes())
        .map_err(|error| match error {
            grt_core::Error::TooLarge { .. } => anyhow::anyhow!(
                "{error}: too many refs or packs for a version 1 manifest; \
                 delete stale branches or tags and try again"
            ),
            other => other.into(),
        })?;
    save(repo, &signed)?;

    writeln!(
        out,
        "Signed manifest #{} with {} ref(s) and {} pack(s){}.",
        signed.seq().get(),
        manifest.refs().len(),
        manifest.packs().len(),
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
    for mirror in manifest.mirrors() {
        writeln!(out, "Mirror: {mirror}")?;
    }
    Ok(signed)
}

/// The next manifest's refs, `HEAD`, upstream and mirrors, without packs.
///
/// A plain sign publishes the local branches and tags. A sign with
/// `--update`/`--delete` (a push) changes only those refs of `previous`.
fn refs_manifest(
    repo: &Repo,
    args: &SignArgs,
    previous: Option<&SignedManifest>,
) -> anyhow::Result<Manifest> {
    let mut manifest = Manifest::new();
    let mut head = None;
    if args.is_update() {
        // A push: the published refs, changed only where the push changed them.
        let mut refs = BTreeMap::new();
        if let Some(previous) = previous {
            refs.clone_from(previous.manifest().refs());
            head = previous.manifest().head().cloned();
        }
        for name in &args.deletes {
            refs.remove(name);
        }
        for (name, id) in &args.updates {
            refs.insert(name.clone(), *id);
        }
        for (name, id) in refs {
            manifest.insert_ref(name, id)?;
        }
    } else {
        for (name, id) in repo.refs(PUBLISHED_REFS)? {
            manifest.insert_ref(name, id)?;
        }
    }
    if head
        .as_ref()
        .is_none_or(|head| !manifest.refs().contains_key(head))
    {
        head = repo.head_target()?;
    }
    if let Some(head) = head
        && manifest.refs().contains_key(&head)
    {
        manifest.set_head(Some(head))?;
    }
    let upstream = match &args.upstream {
        Some(upstream) => Some(upstream.clone()),
        None => origin_upstream(repo)?.or_else(|| {
            // A push to a pure swarm remote keeps the recorded upstream.
            previous
                .as_ref()
                .filter(|_| args.is_update())
                .and_then(|previous| previous.manifest().upstream().map(str::to_owned))
        }),
    };
    manifest.set_upstream(upstream);

    manifest.set_mirrors(match (previous, args.mirrors.is_empty()) {
        (Some(previous), true) => previous.manifest().mirrors().to_vec(),
        _ => args.mirrors.clone(),
    })?;
    Ok(manifest)
}

/// Writes a pack of the objects reachable from the manifest's refs but not
/// from `exclude`, and stores it under its infohash. Returns `None` when
/// there is nothing new to pack.
async fn write_next_pack(
    repo: &Repo,
    manifest: &Manifest,
    exclude: &[ObjectId],
) -> anyhow::Result<Option<(InfoHash, u32)>> {
    let tips: Vec<ObjectId> = manifest
        .refs()
        .values()
        .copied()
        .collect::<BTreeSet<ObjectId>>()
        .into_iter()
        .collect();
    let staging = packs_dir(repo).join(format!("staging-{}", std::process::id()));
    fs::create_dir_all(&staging)?;
    let file = staging.join(PACK_FILE);
    let objects = repo.write_pack(&tips, exclude, &file)?;
    if objects == 0 {
        fs::remove_dir_all(&staging)?;
        return Ok(None);
    }
    let pack = grt_swarm::info_hash_of(&file).await?;
    let stored = packs_dir(repo).join(pack.to_string());
    if stored.exists() {
        fs::remove_dir_all(&staging)?;
    } else {
        fs::rename(&staging, &stored)?;
    }
    Ok(Some((pack, objects)))
}

/// Where the pack with infohash `pack` is stored.
pub(crate) fn pack_path(repo: &Repo, pack: InfoHash) -> PathBuf {
    packs_dir(repo).join(pack.to_string()).join(PACK_FILE)
}

fn packs_dir(repo: &Repo) -> PathBuf {
    repo.git_dir().join("torrent").join("packs")
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
            "{path} failed verification ({error}).\n\
             If it was signed by git-remote-torrent 0.1.0, the signature format changed \
             in 0.2.0: delete it and run `git torrent sign` again. Otherwise it may have \
             been tampered with.",
            path = path.display()
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
