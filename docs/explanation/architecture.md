# Architecture

| Crate | Responsibility | Async |
|-------|----------------|-------|
| `grt-core` | Manifest schema (bencode), Ed25519 signing, trust store, URL parsing | no |
| `grt-git` | Wrapper over git plumbing via the installed `git` binary | no |
| `grt-swarm` | Mainline DHT (BEP 44/46), BitTorrent v2 packs, BEP 19 web seeds, mDNS | yes |
| `grt-upstream` | Canonical HTTPS/SSH remote; forge key discovery | yes |
| `grt-helper` | `git-remote-torrent`: the remote-helper protocol | yes |
| `grt-cli` | `git-torrent`: init, keygen, trust, seed, status, compact | yes |
| `grt-daemon` | `git-torrentd`: background seeder, push webhooks, mDNS announce | yes |

## Data model

A **signed ref manifest** is a BEP 44 mutable item, keyed by the publisher's Ed25519 key:

```
{ v, seq, upstream_url, head, refs | refs_root, packs: [infohash, …] }
```

`packs` is an ordered chain of self-contained packfiles. Each one is distributed as a
BitTorrent v2 torrent. `git torrent compact` folds the chain back into one base pack.

## Hybrid fetch

1. Query the upstream (`ls-remote`) and the DHT manifest in parallel.
2. If the upstream agrees with the manifest, download packs from the swarm and web seeds,
   and fall back to the upstream on timeout.
3. If the upstream is gone, serve from the last verified manifest and the swarm, and tell
   the user.
4. If they disagree, the higher `seq` or the upstream wins. Both sets of refs are exposed
   under `refs/torrent/*`.
