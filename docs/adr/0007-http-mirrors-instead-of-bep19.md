# 0007. HTTP mirrors verified by infohash, instead of BEP 19 web seeds

- Status: Accepted
- Date: 2026-10-25

## Context and problem

The roadmap planned BEP 19 web seeds: HTTP URLs in the torrent's `url-list` that a
client downloads pieces from alongside peers. They let any static file host back the
swarm, so a fetch still works when every seeder is offline. That is the last step of the
kill switch.

`librqbit` 9 has no web seed support: none of its crates read `url-list` or fetch pieces
over HTTP (ADR-0006 chose `librqbit` for the transport).

## Considered options

1. **Implement BEP 19 inside `librqbit`** (fork or upstream contribution). This gives
   piece-level mixing of HTTP and peers, but it is a large change in another project's
   codebase, and 0.2 would wait on its review.
2. **Our own HTTP mirror fallback, with mirrors listed in the signed manifest.** Download
   whole packs from `<base>/<infohash>.pack`, and accept one only if it hashes to the
   manifest's infohash.
3. **The same fallback with mirrors set only in client config.** No format change, but a
   new cloner must learn the mirror URL some other way, which defeats the kill switch.

## Decision

Option 2.

- **Format:** `grt-core` adds an optional `ws` key to manifest format 1: a list of
  `http://`/`https://` base URLs (each at most 200 bytes). Older readers ignore unknown
  keys, so this is a minor format change.
- **Fetching:** `grt_swarm::fetch_from_mirror` downloads `<base>/<infohash>.pack`, hashes
  it as a torrent named `pack` would be, and deletes it unless the infohash matches. A
  mirror is therefore no more trusted than a peer. Downloads are capped at 4 GiB.
- **Order:** the helper tries peers first. When the manifest or `torrent.mirror` provides
  mirrors, peers get at most 30 seconds per pack, then each mirror is tried in turn.
- **Publishing:** `git torrent export <dir>` lays the chain out for upload to any static
  host. `git torrent sign --mirror <url>` signs the base URL in, and later signs keep it.

## Consequences

- Any static host can keep a repository cloneable: GitHub Pages, an object store, a
  release's assets or a forge's package registry. It needs no torrent client and no
  trust.
- A pack comes entirely from one source; transfers can't mix peers and HTTP within a
  pack. Packs are per push and usually small, so this matters little.
- Mirror URLs cost manifest bytes, which come out of the 1000-byte budget shared with
  refs.
- If `librqbit` gains BEP 19, mirrors could also be added to the torrents' `url-list`.
  The `ws` field stays useful either way, because torrents are fetched by infohash alone
  and carry no `url-list` of their own.
