# Architecture

| Crate | Responsibility | Async |
|-------|----------------|-------|
| `grt-core` | Manifest schema (bencode), Ed25519 signing, trust store, URL parsing | no |
| `grt-git` | Wrapper over git plumbing via the installed `git` binary | no |
| `grt-swarm` | Mainline DHT (BEP 44/46), hybrid v1/v2 pack torrents, HTTP mirrors, local discovery | yes |
| `grt-upstream` | Canonical HTTPS/SSH remote; forge key discovery | yes |
| `grt-helper` | `git-remote-torrent`: the remote-helper protocol | yes |
| `grt-cli` | `git-torrent`: key, trust, sign, seed, publish, republish, adopt, export, verify-mirror, compact, reconcile, status | yes |

## Data model

A **signed ref manifest** is a BEP 44 mutable item, keyed by the publisher's Ed25519
key and an optional salt:

```
{ v, refs: {name: oid}, packs: <v1 infohash…>, packs2?, head?, up? }   signed with (salt, seq)
```

See the [manifest format reference](../reference/manifest-format.md).

`packs` is an ordered chain of self-contained packfiles. Each one is a BEP 52 hybrid
torrent, transferred over BitTorrent v1
([ADR-0006](../adr/0006-bittorrent-v1-and-bep44-value-layout.md)). The manifest also
signs each pack's SHA-256 (v2) infohash in `packs2`, which is checked after every
download ([ADR-0008](../adr/0008-hybrid-torrents-and-sha256.md)).
`git torrent compact` folds the chain back into one base pack, and `sign` does so by
itself when the next pack would push the manifest past 1000 bytes.

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

## Push

The helper is short-lived, so it never seeds. `Router::push` sends the push to the
upstream (hybrid) or checks it for fast-forwards against the manifest (pure P2P). Then,
if `remote.<name>.torrentPublish` is set, it runs `git torrent sign --update/--delete`
with the accepted refs, which extends the pack chain and saves the new manifest to
`.git/torrent/manifest`. Signing stays in one place, including the passphrase prompt.

A running `git torrent seed` (it writes `.git/torrent/seed.pid`) polls that file. It seeds
new packs *before* publishing, so a published manifest never names a pack that no one
has, and it publishes with the previous `seq` as the BEP 44 `cas`.

If the upstream push fails outright (unreachable, deleted, access revoked) and the remote
publishes, the router pushes to the swarm only, with the fast-forward rule, and records
the refs in `.git/torrent/unsynced`. `git torrent reconcile` later pushes their manifest
values to the upstream with plain `git push --porcelain`, without force. It clears only
the refs the upstream accepted, so a conflict stays visible until it is merged.
