# 0008. Hybrid v1/v2 torrents: every pack verified with SHA-256

- Status: Accepted
- Date: 2026-10-26
- Extends: [ADR-0006](0006-bittorrent-v1-and-bep44-value-layout.md) (fills its reserved `packs2`)

## Context and problem

ADR-0006 chose BitTorrent v1 for packs, because `librqbit` speaks only v1, and reserved
`packs2` for SHA-256 (v2) infohashes. Packs were identified only by SHA-1. That is
acceptable on its own, since an attacker would need a SHA-1 second preimage. But it left
mirrors, peers and future CI tooling with a single, aging hash.

## Considered options

1. **BEP 52 hybrid torrents.** One info dictionary carries both the v1 metadata (SHA-1
   pieces) and the v2 metadata (a SHA-256 merkle tree over 16 KiB blocks). It has two
   infohashes: SHA-1 and SHA-256 of the same bytes. v1 clients ignore the v2 keys.
2. **A plain SHA-256 per pack** in a new manifest field. It is simpler, but duplicates
   what `packs2` is for and doesn't prepare for a v2 swarm.
3. **Leave packs SHA-1 only.**

A spike settled whether option 1 works with our v1-only engine:

- Our hybrid torrents match libtorrent 2.1.1's byte for byte. The v1 infohash, the v2
  infohash and the pieces root all agree for packs of one block, an odd number of blocks,
  and several pieces, at two piece lengths.
- `librqbit` 9 parses libtorrent's hybrid torrents, seeds ours, and a second session
  fetched one by v1 infohash alone. The BEP 9 metadata exchange carries the v2 keys
  intact, and the downloaded bytes re-derive the same SHA-256 infohash.

## Decision

Option 1.

- **Packs are hybrid torrents**, built in `grt_swarm::hybrid_torrent`. The single file is
  always named `pack`. The piece length is a function of the size: 256 KiB, doubled
  until there are at most 1024 pieces. Anyone can therefore recompute both infohashes
  from a pack's bytes.
- **Writers always fill `packs2`.** `packs` holds the hybrid torrents' v1 infohashes,
  and `packs2` holds their v2 infohashes, in the same order.
- **Readers check both hashes** of every pack, whether it came from peers (on top of
  librqbit's SHA-1 piece checks) or from an HTTP mirror, whenever the manifest has
  `packs2`.
- **Upgrade:** a chain signed before this has no `packs2`, so the next `sign` rebuilds
  it as one hybrid pack, as a compaction does.

## Consequences

- Every pack is bound to the signed manifest by SHA-256, from every source.
- Each pack costs 52 manifest bytes instead of 20, so chains compact sooner (about 15
  packs, depending on refs).
- **Breaking for mirrors:** a hybrid torrent's v1 infohash differs from a plain v1
  torrent's of the same bytes. Clients older than this change recompute a plain v1
  torrent, so they reject packs from mirrors signed after it. Fetching from peers by
  infohash still works for them.
- If a v2-capable engine is adopted, peers can verify each 16 KiB block with SHA-256 and
  find packs by their v2 infohash, with no format change.
