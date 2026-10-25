# 0006. BitTorrent v1 for packs, and the manifest signed as a byte string

- Status: Accepted
- Date: 2026-10-25
- Supersedes: [ADR-0003](0003-non-deterministic-packs.md) (its v2 transport choice only)

## Context and problem

A spike tested the planned swarm stack before any of it was built. It used `librqbit`
9.0.1 for torrents and `mainline` 8.0.1 for the DHT, and it surfaced two problems.

1. **`librqbit` only supports BitTorrent v1.** `create_torrent` builds only
   `TorrentMetaV1Owned`, and pieces are verified with SHA-1. BEP 52 appears only in
   metadata parse errors. ADR-0003 assumed v2 torrents with SHA-256 merkle trees.
2. **Our BEP 44 signatures were in a different layout from `mainline`'s.** `grt-core`
   signed the bencoded manifest dictionary *as* `v` (`1:vd…e`). `mainline` treats `v` as
   a byte string and signs `1:v<len>:<value>`. Storing nodes rejected our items without
   replying, so puts timed out. BEP 44 allows either layout, but only one interoperates.

## Considered options

For the transport:

1. **v1 torrents via `librqbit`.** This works today: the spike moved a real pack between
   two sessions, and git accepted it with `git index-pack --strict`.
2. **v2 via libtorrent-rasterbar (C++ bindings).** This adds a C++ toolchain to CI and
   to the static musl release builds.
3. **v2 implemented ourselves.** This means BEP 52 merkle trees, piece layers and
   hash-request messages: weeks of work.

## Decision

**Transfer packs as BitTorrent v1 torrents.** The manifest's `packs` field holds 20-byte
v1 infohashes.

v2's advantage, SHA-256 hashing instead of SHA-1, doesn't change our security:

- Repository integrity comes from the signed refs, plus git checking every object against
  its id (`index-pack`, connectivity).
- Only the publisher chooses pack bytes, and the publisher is already trusted.
- Anyone else would need a SHA-1 *second preimage*, which isn't feasible.
- What v2 would add is faster detection of corrupt downloads (per 16 KiB block instead of
  per piece), and cross-torrent dedup that our unique-per-push packs wouldn't benefit from.

**Reserve an optional `packs2` field** for 32-byte v2 infohashes of the *same* packs, in
the same order. Readers validate it when it's present. Writers leave it out until a
v2-capable engine is adopted. Adding it then is a minor format change, because readers
already ignore keys they don't use.

**Sign the manifest as a bencode byte string:** `…1:v<len>:<manifest>`. This is the
layout `mainline` uses, and it reproduces the BEP 44 test vectors literally. The length
prefix counts toward the 1000-byte limit.

The manifest format stays at `v = 1`. Version 0.1.0 never published a manifest to any
swarm. It only wrote local caches (`.git/torrent/manifest`), so format 1 is redefined
here rather than bumped. Caches signed by 0.1.0 no longer verify. `git torrent sign`
explains how to clear them.

## Consequences

- The Phase 2 swarm stack (`librqbit` + `mainline`) is proven end to end by the spike:
  torrent transfer, BEP 44 put on a testnet, get from another node, and `grt-core`
  verification.
- Each pack id costs 20 bytes instead of 32, so more packs fit in a manifest.
- The change ships in 0.2.0 as a breaking change to the manifest format and to
  signatures.
- If `librqbit` (or another Rust engine) gains v2 support, publishers can add `packs2`
  without breaking older readers.
