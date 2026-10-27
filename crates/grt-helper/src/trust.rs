//! Which publisher keys a repository takes refs from (threat model H2).
//!
//! While the upstream answers, its refs are canonical and git's object ids
//! keep the swarm honest, so any key will do. When refs come from a manifest
//! instead, the key decides what code arrives. It must then be in the
//! repository's trust store (`git torrent trust`) if that has entries, and
//! otherwise be the key pinned for the upstream the first time a fetch
//! succeeded against it, like SSH's `known_hosts`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use grt_core::{PublicKey, TRUSTED_KEYS_FILE, TrustStore};

use crate::error::Error;

/// `.git/torrent/pins`: one `<hex key> <upstream URL>` line per upstream.
const PINS_FILE: &str = "pins";

/// Says on stderr which key signs the refs about to be served, then checks
/// that `key` may serve them for `upstream` (`None` for `torrent://`
/// remotes) in the repository at `git_dir`.
pub(crate) fn check(
    git_dir: Option<&Path>,
    key: &PublicKey,
    upstream: Option<&str>,
) -> Result<(), Error> {
    eprintln!(
        "note: refs are signed by {} (torrent://{key})",
        key.to_openssh("").trim_end()
    );
    let Some(git_dir) = git_dir else {
        return Ok(());
    };
    let store = trust_store(git_dir)?;
    if store.contains(key) {
        // Trusting a new key also moves the upstream's pin to it.
        if let Some(upstream) = upstream {
            let mut pins = pins(git_dir)?;
            if pins.insert(upstream.to_owned(), *key) != Some(*key) {
                save_pins(git_dir, &pins)?;
            }
        }
        return Ok(());
    }
    if !store.is_empty() {
        return Err(Error::Untrusted(format!(
            "key {key} is not in this repository's trusted keys; if you trust it, run \
             `git torrent trust add {key}`"
        )));
    }
    if let Some(upstream) = upstream
        && let Some(pinned) = pins(git_dir)?.get(upstream)
        && pinned != key
    {
        return Err(Error::Untrusted(format!(
            "{upstream} was first fetched with key {pinned}, but this manifest is signed by \
             {key}; if the publisher really changed keys, run `git torrent trust add {key}`"
        )));
    }
    Ok(())
}

/// Pins `key` for `upstream` unless the upstream has a pin already, after a
/// fetch the live upstream answered.
pub(crate) fn pin(git_dir: &Path, key: &PublicKey, upstream: &str) -> Result<(), Error> {
    let mut pins = pins(git_dir)?;
    match pins.get(upstream) {
        Some(pinned) if pinned == key => {}
        Some(pinned) => eprintln!(
            "warning: {upstream} is pinned to key {pinned}, not {key}, so manifests signed by \
             {key} will be refused while the upstream is unavailable; if the publisher \
             changed keys, run `git torrent trust add {key}`"
        ),
        None => {
            pins.insert(upstream.to_owned(), *key);
            save_pins(git_dir, &pins)?;
        }
    }
    Ok(())
}

fn trust_store(git_dir: &Path) -> Result<TrustStore, Error> {
    match read(&git_dir.join("torrent").join(TRUSTED_KEYS_FILE))? {
        Some(text) => Ok(text.parse()?),
        None => Ok(TrustStore::new()),
    }
}

fn pins(git_dir: &Path) -> Result<BTreeMap<String, PublicKey>, Error> {
    let path = pins_path(git_dir);
    let mut pins = BTreeMap::new();
    for line in read(&path)?.unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parsed = line
            .split_once(' ')
            .and_then(|(key, upstream)| Some((key.parse().ok()?, upstream)));
        let Some((key, upstream)) = parsed else {
            return Err(Error::Untrusted(format!(
                "{} has a malformed line: {line:?}",
                path.display()
            )));
        };
        pins.insert(upstream.to_owned(), key);
    }
    Ok(pins)
}

fn save_pins(git_dir: &Path, pins: &BTreeMap<String, PublicKey>) -> Result<(), Error> {
    let lines = pins
        .iter()
        .map(|(upstream, key)| format!("{key} {upstream}\n"));
    let text: String = std::iter::once(
        "# git-remote-torrent: the publisher key first fetched for each upstream\n".to_owned(),
    )
    .chain(lines)
    .collect();
    let path = pins_path(git_dir);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, text)?;
    Ok(())
}

fn pins_path(git_dir: &Path) -> PathBuf {
    git_dir.join("torrent").join(PINS_FILE)
}

fn read(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use grt_core::PublisherKey;

    use super::*;

    const UPSTREAM: &str = "https://forge.example/alice/project.git";

    fn key() -> PublicKey {
        PublisherKey::generate().unwrap().public_key()
    }

    #[test]
    fn the_first_key_is_pinned_and_another_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let (first, second) = (key(), key());
        check(Some(dir.path()), &first, Some(UPSTREAM)).unwrap();
        pin(dir.path(), &first, UPSTREAM).unwrap();
        pin(dir.path(), &second, UPSTREAM).unwrap();
        check(Some(dir.path()), &first, Some(UPSTREAM)).unwrap();
        let error = check(Some(dir.path()), &second, Some(UPSTREAM)).unwrap_err();
        assert!(
            error.to_string().contains("first fetched with key"),
            "{error}"
        );
        // Pins are per upstream; torrent:// remotes name their key themselves.
        check(
            Some(dir.path()),
            &second,
            Some("https://elsewhere.example/x.git"),
        )
        .unwrap();
        check(Some(dir.path()), &second, None).unwrap();
    }

    #[test]
    fn a_trust_store_with_entries_decides() {
        let dir = tempfile::TempDir::new().unwrap();
        let (trusted, other) = (key(), key());
        pin(dir.path(), &other, UPSTREAM).unwrap();
        let mut store = TrustStore::new();
        store.insert(trusted, "alice");
        fs::write(
            dir.path().join("torrent").join(TRUSTED_KEYS_FILE),
            store.to_string(),
        )
        .unwrap();

        let error = check(Some(dir.path()), &other, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("not in this repository's trusted keys"),
            "{error}"
        );
        // A trusted key overrides the pin and replaces it.
        check(Some(dir.path()), &trusted, Some(UPSTREAM)).unwrap();
        assert_eq!(pins(dir.path()).unwrap()[UPSTREAM], trusted);
    }
}
