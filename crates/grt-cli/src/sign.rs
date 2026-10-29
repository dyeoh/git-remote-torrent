use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use grt_core::{
    InfoHash, InfoHashV2, Manifest, ObjectId, PublisherKey, RefName, RemoteUrl, Seq, SignedManifest,
};
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
    runtime.block_on(sign_next(&repo, &key, args, Chain::Extend, out))?;
    Ok(())
}

/// How the next manifest's pack chain is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chain {
    /// Append one pack holding the objects the previous chain lacks.
    Extend,
    /// Replace the chain with one full pack.
    Compact,
}

/// Signs the next manifest for the local refs and caches it.
///
/// If the refs reach objects the previous manifest's packs do not hold, the
/// chain grows by one pack first (ADR-0004), so every signed manifest's packs
/// contain its refs. When the extended manifest no longer fits in a BEP 44
/// item, or with [`Chain::Compact`], the chain is replaced by one full pack.
pub(crate) async fn sign_next(
    repo: &Repo,
    key: &PublisherKey,
    args: &SignArgs,
    chain: Chain,
    out: &mut dyn Write,
) -> anyhow::Result<SignedManifest> {
    // Packs written from a cut-off history would lack the parents clients need.
    if repo.is_shallow()? {
        bail!(
            "this is a shallow clone, so its packs would be missing history; \
             run `git fetch --unshallow` first (in GitHub Actions: `fetch-depth: 0`)"
        );
    }
    // The chain continues only for the same key and name; anything else starts over.
    let previous = load(repo)?.filter(|previous| {
        previous.public_key() == key.public_key() && previous.salt() == args.name.as_bytes()
    });

    let refs = refs_manifest(repo, args, previous.as_ref(), out)?;
    let seq = match &previous {
        Some(previous) => previous.seq().next()?,
        None => Seq::new(1),
    };
    // The signed time lets cloners judge how stale a manifest is (threat model M3).
    let now = unix_now()?;
    let sign = |manifest: &Manifest| key.sign(&at(manifest, Some(now)), seq, args.name.as_bytes());

    let mut signed = None;
    // A chain signed before 0.4 has no SHA-256 hashes, so it is rebuilt once.
    let previous_chain = match &previous {
        Some(previous) => hybrid_chain(previous.manifest()),
        None => Some(Vec::new()),
    };
    if chain == Chain::Extend && previous_chain.is_none() {
        writeln!(
            out,
            "Rebuilding the pack chain as one SHA-256-verified (hybrid) pack."
        )?;
    }
    // Nothing changed: keep the published manifest rather than bumping its
    // sequence number, so restarting a seeder doesn't churn the address.
    if chain == Chain::Extend
        && let (Some(previous), Some(packs)) = (&previous, &previous_chain)
        && with_packs(&refs, packs) == at(previous.manifest(), None)
    {
        writeln!(
            out,
            "Manifest #{} is up to date ({} ref(s), {} pack(s)).",
            previous.seq().get(),
            previous.manifest().refs().len(),
            previous.manifest().packs().len()
        )?;
        print_address(previous, out)?;
        return Ok(previous.clone());
    }
    if chain == Chain::Extend
        && let Some(mut packs) = previous_chain
    {
        let exclude: Vec<ObjectId> = previous
            .as_ref()
            .map(|previous| previous.manifest().refs().values().copied().collect())
            .unwrap_or_default();
        if let Some((pack, objects)) = write_next_pack(repo, &refs, &exclude).await? {
            writeln!(
                out,
                "Added pack {} ({objects} objects) to the chain.",
                pack.0
            )?;
            packs.push(pack);
        }
        match sign(&with_packs(&refs, &packs)) {
            Ok(extended) => signed = Some(extended),
            // Each pack costs 52 bytes of ids; one pack frees the room.
            Err(grt_core::Error::TooLarge { .. }) if packs.len() > 1 => writeln!(
                out,
                "The manifest is full with {} packs; compacting the chain.",
                packs.len()
            )?,
            Err(error) => return Err(too_large_hint(error)),
        }
    }
    let signed = if let Some(signed) = signed {
        signed
    } else {
        let packs = match write_next_pack(repo, &refs, &[]).await? {
            Some((pack, objects)) => {
                writeln!(
                    out,
                    "Compacted the chain into pack {} ({objects} objects).",
                    pack.0
                )?;
                vec![pack]
            }
            None => Vec::new(),
        };
        sign(&with_packs(&refs, &packs)).map_err(too_large_hint)?
    };
    let manifest = signed.manifest();
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
    print_address(&signed, out)?;
    Ok(signed)
}

/// The next manifest's refs, `HEAD`, upstream and mirrors, without packs.
///
/// A plain sign publishes the local branches and tags [`published_refs`]
/// picks. A sign with `--update`/`--delete` (a push) changes only those refs
/// of `previous`.
fn refs_manifest(
    repo: &Repo,
    args: &SignArgs,
    previous: Option<&SignedManifest>,
    out: &mut dyn Write,
) -> anyhow::Result<Manifest> {
    let mut manifest = Manifest::new();
    let mut head = None;
    if args.builds_on_previous() {
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
        for (name, id) in published_refs(repo, args.all, previous, out)? {
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
                .filter(|_| args.builds_on_previous())
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

/// The `torrent://` address `signed` is published at.
pub(crate) fn address(signed: &SignedManifest) -> RemoteUrl {
    RemoteUrl::Swarm {
        key: signed.public_key(),
        salt: signed.salt().to_vec(),
    }
}

/// The local branches and tags a plain sign publishes. With `all`, every
/// one. Otherwise the previous manifest's, the current branch, branches a
/// remote has, and tags on any of those, so work that exists only here
/// stays private: a published pack can't be recalled.
fn published_refs(
    repo: &Repo,
    all: bool,
    previous: Option<&SignedManifest>,
    out: &mut dyn Write,
) -> anyhow::Result<Vec<(RefName, ObjectId)>> {
    let local = repo.refs(PUBLISHED_REFS)?;
    if all {
        return Ok(local);
    }
    let mut wanted: BTreeSet<String> = previous
        .map(|previous| {
            previous
                .manifest()
                .refs()
                .keys()
                .map(|name| name.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default();
    if let Some(head) = repo.head_target()? {
        wanted.insert(head.as_str().to_owned());
    }
    // refs/remotes/<remote>/<branch> means the remote has refs/heads/<branch>.
    for (name, _) in repo.refs(&["refs/remotes"])? {
        if let Some((_, branch)) = name
            .as_str()
            .strip_prefix("refs/remotes/")
            .and_then(|rest| rest.split_once('/'))
        {
            wanted.insert(format!("refs/heads/{branch}"));
        }
    }
    let tips: BTreeSet<ObjectId> = local
        .iter()
        .filter(|(name, _)| {
            name.as_str().starts_with("refs/heads/") && wanted.contains(name.as_str())
        })
        .map(|(_, id)| *id)
        .collect();
    for tip in &tips {
        for (name, _) in repo.refs_merged_into(&["refs/tags"], tip)? {
            wanted.insert(name.as_str().to_owned());
        }
    }
    let (published, skipped): (Vec<_>, Vec<_>) = local
        .into_iter()
        .partition(|(name, _)| wanted.contains(name.as_str()));
    let names = |refs: &[(RefName, ObjectId)]| {
        refs.iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    writeln!(out, "Publishing {}.", names(&published))?;
    if !skipped.is_empty() {
        writeln!(
            out,
            "Not publishing refs that exist only here: {} (`--all` publishes them).",
            names(&skipped)
        )?;
    }
    Ok(published)
}

fn print_address(signed: &SignedManifest, out: &mut dyn Write) -> anyhow::Result<()> {
    writeln!(out, "Address: {}", address(signed))?;
    for mirror in signed.manifest().mirrors() {
        writeln!(out, "Mirror: {mirror}")?;
    }
    Ok(())
}

/// `manifest` with `signed_at` as its time of signing; `None` leaves only
/// its content, to compare.
fn at(manifest: &Manifest, signed_at: Option<u64>) -> Manifest {
    let mut manifest = manifest.clone();
    manifest.set_signed_at(signed_at);
    manifest
}

/// Seconds since the Unix epoch.
pub(crate) fn unix_now() -> anyhow::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("the system clock is before 1970")?
        .as_secs())
}

/// `refs` with `packs` as its chain.
fn with_packs(refs: &Manifest, packs: &[Pack]) -> Manifest {
    let mut manifest = refs.clone();
    for &(pack, _) in packs {
        manifest.push_pack(pack);
    }
    if !packs.is_empty() {
        manifest
            .set_packs_v2(Some(packs.iter().map(|&(_, v2)| v2).collect()))
            .expect("one v2 infohash per pack");
    }
    manifest
}

/// A pack's v1 and SHA-256 (v2) infohashes, as a hybrid torrent (ADR-0008).
type Pack = (InfoHash, InfoHashV2);

/// The manifest's chain with both infohashes, or `None` if it predates
/// hybrid packs and lists only v1 infohashes.
fn hybrid_chain(manifest: &Manifest) -> Option<Vec<Pack>> {
    match manifest.packs_v2() {
        Some(v2) => Some(
            manifest
                .packs()
                .iter()
                .copied()
                .zip(v2.iter().copied())
                .collect(),
        ),
        None if manifest.packs().is_empty() => Some(Vec::new()),
        None => None,
    }
}

fn too_large_hint(error: grt_core::Error) -> anyhow::Error {
    match error {
        grt_core::Error::TooLarge { .. } => anyhow::anyhow!(
            "{error}: too many refs or mirrors for a version 1 manifest; \
             delete stale branches or tags and try again"
        ),
        other => other.into(),
    }
}

/// Writes a pack of the objects reachable from the manifest's refs but not
/// from `exclude`, and stores it under its infohash. Returns `None` when
/// there is nothing new to pack.
async fn write_next_pack(
    repo: &Repo,
    manifest: &Manifest,
    exclude: &[ObjectId],
) -> anyhow::Result<Option<(Pack, u32)>> {
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
    let hashes = grt_swarm::hashes_of(&file).await?;
    let stored = packs_dir(repo).join(hashes.v1.to_string());
    if stored.exists() {
        fs::remove_dir_all(&staging)?;
    } else {
        fs::rename(&staging, &stored)?;
    }
    Ok(Some(((hashes.v1, hashes.v2), objects)))
}

/// Where the pack with infohash `pack` is stored.
pub(crate) fn pack_path(repo: &Repo, pack: InfoHash) -> PathBuf {
    grt_swarm::stored_pack_path(&repo.git_dir().join("torrent"), pack)
}

fn packs_dir(repo: &Repo) -> PathBuf {
    grt_swarm::packs_dir(&repo.git_dir().join("torrent"))
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

/// Reads and verifies a copy of a signed manifest, such as one CI stores
/// next to its packs.
pub(crate) fn read(path: &Path) -> anyhow::Result<SignedManifest> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    SignedManifest::decode(&bytes)
        .with_context(|| format!("{} is not a valid signed manifest", path.display()))
}

pub(crate) fn save(repo: &Repo, signed: &SignedManifest) -> anyhow::Result<()> {
    let path = path(repo);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, signed.encode()).with_context(|| format!("writing {}", path.display()))
}

fn path(repo: &Repo) -> PathBuf {
    repo.git_dir().join("torrent").join("manifest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_without_sha256_are_rebuilt() {
        let mut manifest = Manifest::new();
        assert_eq!(hybrid_chain(&manifest), Some(Vec::new()), "no packs yet");
        manifest.push_pack(InfoHash::from([1; 20]));
        assert_eq!(
            hybrid_chain(&manifest),
            None,
            "a v1-only chain (before 0.4) cannot be extended"
        );
        let v2 = InfoHashV2::from([2; 32]);
        manifest.set_packs_v2(Some(vec![v2])).unwrap();
        assert_eq!(
            hybrid_chain(&manifest),
            Some(vec![(InfoHash::from([1; 20]), v2)]),
            "a hybrid chain pairs each pack's hashes"
        );
        assert_eq!(
            with_packs(&Manifest::new(), &[(InfoHash::from([1; 20]), v2)]).packs_v2(),
            Some([v2].as_slice()),
            "signing writes packs2"
        );
    }
}
