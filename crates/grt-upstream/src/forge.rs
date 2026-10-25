use grt_core::PublicKey;

use crate::error::Error;

/// Largest `.keys` response accepted, in bytes.
const MAX_KEYS_LEN: u64 = 64 * 1024;

/// Keys a forge user publishes, split by whether they can sign manifests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublishedKeys {
    /// The user's Ed25519 keys.
    pub ed25519: Vec<PublicKey>,
    /// How many other keys (RSA, ECDSA, ...) were skipped.
    pub skipped: usize,
}

/// Fetches the SSH keys a user publishes at `<profile>.keys`, the convention
/// GitHub, GitLab, Gitea and Forgejo all follow.
///
/// `profile` is the user's profile URL, such as `https://github.com/alice`.
/// Only HTTPS is accepted: these keys become a trust root.
///
/// # Errors
///
/// Returns [`Error::Forge`] if the URL is not HTTPS or the request fails.
pub fn fetch_published_keys(profile: &str) -> Result<PublishedKeys, Error> {
    let profile = profile.trim_end_matches('/');
    if !profile.starts_with("https://") {
        return Err(Error::Forge(format!("{profile} is not an https:// URL")));
    }
    let url = format!("{profile}.keys");
    let body = ureq::get(&url)
        .call()
        .map_err(|error| Error::Forge(format!("GET {url}: {error}")))?
        .body_mut()
        .with_config()
        .limit(MAX_KEYS_LEN)
        .read_to_string()
        .map_err(|error| Error::Forge(format!("reading {url}: {error}")))?;
    Ok(parse_published_keys(&body))
}

/// Parses a `.keys` body: one OpenSSH public key per line.
pub fn parse_published_keys(body: &str) -> PublishedKeys {
    let mut keys = PublishedKeys::default();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        match PublicKey::from_openssh(line) {
            Ok(key) => keys.ed25519.push(key),
            Err(_) => keys.skipped += 1,
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use grt_core::PublisherKey;

    use super::*;

    #[test]
    fn keeps_ed25519_and_counts_the_rest() {
        let key = PublisherKey::from_seed([3; 32]).public_key();
        let body = format!(
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAAAgQC7\n{}\n\necdsa-sha2-nistp256 AAAAE2VjZHNh\n",
            key.to_openssh("")
        );
        assert_eq!(
            parse_published_keys(&body),
            PublishedKeys {
                ed25519: vec![key],
                skipped: 2
            }
        );
    }

    #[test]
    fn refuses_plain_http() {
        assert!(
            matches!(
                fetch_published_keys("http://github.com/alice"),
                Err(Error::Forge(_))
            ),
            "trust roots must not be fetched over plain HTTP"
        );
    }
}
