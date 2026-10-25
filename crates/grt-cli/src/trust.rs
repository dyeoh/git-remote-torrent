use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use grt_core::{PublicKey, TrustStore};
use grt_git::Repo;

use crate::cli::TrustCommand;
use crate::key;

pub(crate) fn run(command: TrustCommand, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let mut store = load(&repo)?;
    match command {
        TrustCommand::Add { key, label, own } => {
            let (key, comment) = if own {
                (key::signing_public_key()?, "self".to_owned())
            } else {
                parse_key(&key.join(" "))?
            };
            let label = label.unwrap_or(comment);
            if store.insert(key, label) {
                save(&repo, &store)?;
                writeln!(out, "Trusted {key}.")?;
            } else {
                writeln!(out, "{key} is already trusted.")?;
            }
        }
        TrustCommand::Import { profile } => {
            let published = grt_upstream::fetch_published_keys(&profile)?;
            if published.ed25519.is_empty() {
                bail!(
                    "{profile} publishes no Ed25519 keys ({} other keys skipped)",
                    published.skipped
                );
            }
            let added = published
                .ed25519
                .iter()
                .filter(|key| store.insert(**key, profile.trim_end_matches('/')))
                .count();
            save(&repo, &store)?;
            writeln!(
                out,
                "Trusted {added} new Ed25519 key(s) from {profile} ({} already trusted, {} non-Ed25519 skipped).",
                published.ed25519.len() - added,
                published.skipped
            )?;
        }
        TrustCommand::List => {
            if store.is_empty() {
                writeln!(
                    out,
                    "No trusted publishers. Add one with `git torrent trust add`."
                )?;
            }
            for entry in store.iter() {
                writeln!(out, "{}  {}", entry.key, entry.label)?;
            }
        }
        TrustCommand::Remove { key } => {
            let (key, _) = parse_key(&key.join(" "))?;
            if !store.remove(&key) {
                bail!("{key} is not trusted");
            }
            save(&repo, &store)?;
            writeln!(out, "Removed {key}.")?;
        }
    }
    Ok(())
}

/// Loads the repository's trust store; a missing file is an empty store.
pub(crate) fn load(repo: &Repo) -> anyhow::Result<TrustStore> {
    let path = path(repo);
    match fs::read_to_string(&path) {
        Ok(text) => text
            .parse()
            .with_context(|| format!("reading {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TrustStore::new()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save(repo: &Repo, store: &TrustStore) -> anyhow::Result<()> {
    let path = path(repo);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, store.to_string()).with_context(|| format!("writing {}", path.display()))
}

fn path(repo: &Repo) -> PathBuf {
    repo.git_dir().join("torrent").join("trusted-keys")
}

/// Accepts a 64-digit hex key or an OpenSSH public key line; returns the key
/// and the line's comment.
fn parse_key(text: &str) -> anyhow::Result<(PublicKey, String)> {
    let text = text.trim();
    if let Ok(key) = text.parse::<PublicKey>() {
        return Ok((key, String::new()));
    }
    let key = PublicKey::from_openssh(text)
        .with_context(|| format!("{text:?} is not a hex or OpenSSH Ed25519 key"))?;
    let comment = text
        .splitn(3, char::is_whitespace)
        .nth(2)
        .unwrap_or("")
        .trim();
    Ok((key, comment.to_owned()))
}
