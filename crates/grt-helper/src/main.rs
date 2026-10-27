//! `git-remote-torrent`: invoked by git for `torrent::<url>` and `torrent://<key>` remotes.

use std::io;
use std::path::Path;

use anyhow::{Context, bail};
use grt_core::RemoteUrl;
use grt_git::Repo;
use grt_helper::{DhtSwarm, Router, Settings};
use grt_upstream::GitUpstream;

fn main() -> anyhow::Result<()> {
    // Git runs `git-remote-torrent <remote> [<url>]`; with no configured
    // remote, the URL is passed as both arguments or only as the first.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (name, url) = match args.as_slice() {
        [name, url] if name != url => (Some(name.as_str()), url),
        [_, url] | [url] => (None, url),
        _ => bail!("usage: git-remote-torrent <remote> [<url>] (this is run by git, not by hand)"),
    };
    let remote: RemoteUrl = url.parse()?;
    // Git sets GIT_DIR for the helper; `git ls-remote` may run outside a repository.
    let repo = Repo::discover(Path::new(".")).ok();
    let (stdin, stdout) = (io::stdin().lock(), io::stdout().lock());
    match &remote {
        RemoteUrl::Hybrid { upstream } => {
            let swarm = match torrent_address(name)? {
                Some((key, salt)) => Some(
                    DhtSwarm::new(repo, key, &salt, Settings::from_config()?)?
                        .publishing(publishes(name)?)
                        .for_remote(name)
                        .for_upstream(Some(upstream)),
                ),
                None => None,
            };
            let mut router = Router::hybrid(GitUpstream::new(upstream), swarm);
            grt_helper::run(&mut router, stdin, stdout)
        }
        RemoteUrl::Swarm { key, salt } => {
            let swarm = DhtSwarm::new(repo, *key, salt, Settings::from_config()?)?
                .publishing(publishes(name)?)
                .for_remote(name);
            let mut router: Router<GitUpstream, DhtSwarm> = Router::swarm(swarm);
            grt_helper::run(&mut router, stdin, stdout)
        }
        _ => bail!("unsupported remote {remote}"),
    }
    .with_context(|| format!("git-remote-torrent: {remote}"))
}

/// The swarm address backing a hybrid remote, from
/// `remote.<name>.torrentAddress` (a `torrent://` URL).
fn torrent_address(name: Option<&str>) -> anyhow::Result<Option<(grt_core::PublicKey, Vec<u8>)>> {
    let Some(name) = name else {
        return Ok(None);
    };
    let key = format!("remote.{name}.torrentAddress");
    let Some(address) = grt_git::config_all(&key)?.pop() else {
        return Ok(None);
    };
    match address.parse()? {
        RemoteUrl::Swarm { key, salt } => Ok(Some((key, salt))),
        _ => bail!("{key} must be a torrent:// address, not {address:?}"),
    }
}

/// True if pushes to the remote should sign new manifests
/// (`remote.<name>.torrentPublish`).
fn publishes(name: Option<&str>) -> anyhow::Result<bool> {
    let Some(name) = name else {
        return Ok(false);
    };
    let key = format!("remote.{name}.torrentPublish");
    match grt_git::config_all(&key)?.pop().as_deref() {
        None | Some("false" | "no" | "off" | "0") => Ok(false),
        Some("true" | "yes" | "on" | "1") => Ok(true),
        Some(other) => bail!("{key} must be a boolean, not {other:?}"),
    }
}
