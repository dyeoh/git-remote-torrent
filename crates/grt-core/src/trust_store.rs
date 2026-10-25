use std::fmt;
use std::str::FromStr;

use crate::error::Error;
use crate::public_key::PublicKey;

/// The set of publisher keys whose manifests a repository accepts.
///
/// Stored as OpenSSH public key lines, one per key, with the comment used as
/// a human-readable label. Blank lines and `#` comments are ignored on read
/// and dropped on write.
///
/// ```text
/// # git-remote-torrent trusted publishers
/// ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... https://github.com/alice
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrustStore {
    entries: Vec<TrustedKey>,
}

/// One entry in a [`TrustStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    /// The trusted publisher key.
    pub key: PublicKey,
    /// Where the key came from or whose it is.
    pub label: String,
}

impl TrustStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        TrustStore::default()
    }

    /// Returns true if `key` is trusted.
    pub fn contains(&self, key: &PublicKey) -> bool {
        self.entries.iter().any(|entry| entry.key == *key)
    }

    /// Adds `key`. Returns false, leaving the store unchanged, if it was
    /// already trusted.
    pub fn insert(&mut self, key: PublicKey, label: impl Into<String>) -> bool {
        if self.contains(&key) {
            return false;
        }
        self.entries.push(TrustedKey {
            key,
            label: label.into(),
        });
        true
    }

    /// Removes `key`. Returns false if it was not trusted.
    pub fn remove(&mut self, key: &PublicKey) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.key != *key);
        self.entries.len() != before
    }

    /// The trusted keys, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &TrustedKey> {
        self.entries.iter()
    }

    /// The number of trusted keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if no keys are trusted.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FromStr for TrustStore {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let mut store = TrustStore::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let key = PublicKey::from_openssh(line).map_err(|source| Error::InvalidTrustStore {
                line: index + 1,
                source: Box::new(source),
            })?;
            // The comment is everything after the algorithm and base64 fields.
            let label = line
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("")
                .trim();
            store.insert(key, label);
        }
        Ok(store)
    }
}

impl fmt::Display for TrustStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "# git-remote-torrent trusted publishers (OpenSSH public key format)"
        )?;
        for entry in &self.entries {
            writeln!(f, "{}", entry.key.to_openssh(&entry.label))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher_key::PublisherKey;

    fn key(byte: u8) -> PublicKey {
        PublisherKey::from_seed([byte; 32]).public_key()
    }

    #[test]
    fn insert_remove_contains() {
        let mut store = TrustStore::new();
        assert!(store.insert(key(1), "alice"), "new key is added");
        assert!(
            !store.insert(key(1), "alice again"),
            "duplicates are ignored"
        );
        assert!(store.contains(&key(1)));
        assert!(!store.contains(&key(2)));
        assert!(store.remove(&key(1)), "trusted key is removed");
        assert!(!store.remove(&key(1)), "removing twice reports false");
        assert!(store.is_empty());
    }

    #[test]
    fn text_round_trip() {
        let mut store = TrustStore::new();
        store.insert(key(1), "https://github.com/alice");
        store.insert(key(2), "bob laptop");
        let text = store.to_string();
        assert_eq!(
            text.parse::<TrustStore>(),
            Ok(store),
            "labels with spaces survive"
        );
    }

    #[test]
    fn ignores_comments_and_reports_bad_lines() {
        let line = key(1).to_openssh("alice");
        let text = format!("# comment\n\n{line}\n");
        assert_eq!(
            text.parse::<TrustStore>().map(|s| s.len()),
            Ok(1),
            "comments and blanks skipped"
        );

        let bad = format!("{line}\nssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQC7 x\n");
        assert!(
            matches!(
                bad.parse::<TrustStore>(),
                Err(Error::InvalidTrustStore { line: 2, .. })
            ),
            "the offending line number is reported"
        );
    }
}
