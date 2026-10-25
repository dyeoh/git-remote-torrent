//! `git-remote-torrent`: invoked by git for `torrent::<url>` and `torrent://<key>` remotes.

use std::io;

use anyhow::{Context, bail};
use grt_core::RemoteUrl;
use grt_upstream::GitUpstream;

fn main() -> anyhow::Result<()> {
    // Git runs `git-remote-torrent <remote> [<url>]`; with no configured
    // remote, the URL is passed as both arguments or only as the first.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ([_, url] | [url]) = args.as_slice() else {
        bail!("usage: git-remote-torrent <remote> [<url>] (this is run by git, not by hand)");
    };
    let remote: RemoteUrl = url.parse()?;
    match &remote {
        RemoteUrl::Hybrid { upstream } => {
            let upstream = GitUpstream::new(upstream);
            grt_helper::run(&upstream, io::stdin().lock(), io::stdout().lock())
                .with_context(|| format!("git-remote-torrent: {remote}"))?;
        }
        RemoteUrl::Swarm { .. } => {
            bail!("git-remote-torrent: pure P2P remotes (torrent://) are not supported yet")
        }
        _ => bail!("git-remote-torrent: unsupported remote {remote}"),
    }
    Ok(())
}
