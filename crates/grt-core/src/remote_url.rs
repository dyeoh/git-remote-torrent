use std::fmt;
use std::str::FromStr;

use crate::error::Error;
use crate::public_key::PublicKey;
use crate::signed_manifest::MAX_SALT_LEN;

const SWARM_SCHEME: &str = "torrent://";

/// The address git hands to `git-remote-torrent`.
///
/// For a remote written `torrent::<url>`, git passes `<url>` and the helper
/// runs in hybrid mode. For `torrent://<key>[/<name>]`, git passes the whole
/// URL and the helper runs in pure P2P mode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RemoteUrl {
    /// A canonical upstream git URL, accelerated and backed up by the swarm.
    Hybrid {
        /// Any URL git itself understands (`https://`, `ssh://`, `file://`, scp-style).
        upstream: String,
    },
    /// A repository addressed only by its publisher key.
    Swarm {
        /// The publisher's key.
        key: PublicKey,
        /// The repository name under that key, used as the BEP 44 salt. Empty
        /// for a key's only repository.
        salt: Vec<u8>,
    },
}

impl FromStr for RemoteUrl {
    type Err = Error;

    fn from_str(url: &str) -> Result<Self, Error> {
        let invalid = |reason| Error::InvalidUrl {
            url: url.to_owned(),
            reason,
        };
        if let Some(rest) = url.strip_prefix(SWARM_SCHEME) {
            let (key, name) = rest.split_once('/').unwrap_or((rest, ""));
            let key = key
                .parse()
                .map_err(|_| invalid("expected a 64-digit hex Ed25519 key"))?;
            if name.len() > MAX_SALT_LEN {
                return Err(invalid("repository name is longer than 64 bytes"));
            }
            return Ok(RemoteUrl::Swarm {
                key,
                salt: name.as_bytes().to_vec(),
            });
        }
        if url.is_empty() {
            return Err(invalid("is empty"));
        }
        // The upstream URL becomes an argument to `git`; a leading `-` would be
        // read as an option (cf. CVE-2017-1000117).
        if url.starts_with('-') {
            return Err(invalid("starts with `-`"));
        }
        if url.starts_with("torrent::") {
            return Err(invalid("nests `torrent::` inside `torrent::`"));
        }
        Ok(RemoteUrl::Hybrid {
            upstream: url.to_owned(),
        })
    }
}

impl fmt::Display for RemoteUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteUrl::Hybrid { upstream } => write!(f, "torrent::{upstream}"),
            RemoteUrl::Swarm { key, salt } if salt.is_empty() => write!(f, "{SWARM_SCHEME}{key}"),
            RemoteUrl::Swarm { key, salt } => {
                write!(f, "{SWARM_SCHEME}{key}/{}", String::from_utf8_lossy(salt))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";

    #[test]
    fn parses_hybrid_urls() {
        for upstream in [
            "https://github.com/org/repo.git",
            "ssh://git@github.com/org/repo.git",
            "git@github.com:org/repo.git",
            "file:///srv/repo.git",
        ] {
            let url: RemoteUrl = upstream.parse().unwrap();
            assert_eq!(
                url,
                RemoteUrl::Hybrid {
                    upstream: upstream.to_owned()
                },
                "{upstream} is passed through unchanged"
            );
            assert_eq!(url.to_string(), format!("torrent::{upstream}"));
        }
    }

    #[test]
    fn parses_swarm_urls() {
        let bare: RemoteUrl = format!("torrent://{KEY}").parse().unwrap();
        assert!(
            matches!(&bare, RemoteUrl::Swarm { salt, .. } if salt.is_empty()),
            "no name means an empty salt"
        );
        let named: RemoteUrl = format!("torrent://{KEY}/my-repo").parse().unwrap();
        assert!(
            matches!(&named, RemoteUrl::Swarm { salt, .. } if salt == b"my-repo"),
            "the name becomes the salt"
        );
        assert_eq!(named.to_string(), format!("torrent://{KEY}/my-repo"));
    }

    #[test]
    fn rejects_bad_urls() {
        let long_name = format!("torrent://{KEY}/{}", "x".repeat(65));
        for url in [
            "",
            "--upload-pack=touch /tmp/pwned",
            "torrent::https://example.com/r.git",
            "torrent://not-a-key",
            long_name.as_str(),
        ] {
            assert!(
                url.parse::<RemoteUrl>().is_err(),
                "{url:?} must be rejected"
            );
        }
    }
}
