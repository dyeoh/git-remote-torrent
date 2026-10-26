# Architecture

| Crate | Responsibility | Async |
|-------|----------------|-------|
| `grt-core` | Manifest schema (bencode), Ed25519 signing, trust store, URL parsing | no |
| `grt-git` | Wrapper over git plumbing via the installed `git` binary | no |
| `grt-swarm` | Mainline DHT (BEP 44/46), BitTorrent v1 packs, BEP 19 web seeds, mDNS | yes |
| `grt-upstream` | Canonical HTTPS/SSH remote; forge key discovery | yes |
| `grt-helper` | `git-remote-torrent`: the remote-helper protocol | yes |
| `grt-cli` | `git-torrent`: init, keygen, trust, seed, status, compact | yes |
| `grt-daemon` | `git-torrentd`: background seeder, push webhooks, mDNS announce | yes |

## Data model

A **signed ref manifest** is a BEP 44 mutable item, keyed by the publisher's Ed25519
key and an optional salt:

```
{ v, refs: {name: oid}, packs: <v1 infohash…>, packs2?, head?, up? }   signed with (salt, seq)
```

See the [manifest format reference](../reference/manifest-format.md).

`packs` is an ordered chain of self-contained packfiles. Each one is distributed as a
BitTorrent v1 torrent ([ADR-0006](../adr/0006-bittorrent-v1-and-bep44-value-layout.md)). `git torrent compact` folds the chain back into one base pack.

Each pack holds the objects reachable from the new tips but not from the previous
manifest's tips (`Repo::write_pack`). Receivers index the chain oldest first with
`git index-pack --strict` (`Repo::index_pack`). That rejects any pack that links to
objects not yet present, so packs can't be applied out of order.

## Hybrid fetch

`grt_helper::Router` decides, per command, where refs and objects come from:

1. **`list`** asks the upstream (`git ls-remote`). While it answers, its refs are
   canonical, even if a manifest disagrees.
2. **`fetch`** resolves the manifest from the DHT. If the manifest's ref tips cover every
   object git asked for, the pack chain is downloaded from the swarm. If the swarm fails,
   or the upstream is ahead of the manifest, the upstream serves the fetch instead.
3. If the upstream can't list its refs (deleted, private, offline), the manifest serves
   both `list` and `fetch`, and the user is told on stderr.

The last manifest fetched is kept in `.git/torrent/fetched/`. It is used when the DHT is
unreachable, and an older manifest from the DHT never replaces it, which blocks rollback.
Packs already applied are recorded in `.git/torrent/applied/`, so a fetch downloads only
the new end of the chain.

Pure P2P remotes (`torrent://`) follow step 3 always. Exposing an upstream and a manifest
that disagree side by side (`refs/torrent/*`) is planned.
