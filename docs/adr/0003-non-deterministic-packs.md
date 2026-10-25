# 0003. Packs need not be deterministic

- Status: Accepted; transport choice superseded by [0006](0006-bittorrent-v1-and-bep44-value-layout.md)
- Date: 2026-10-25

## Context and problem

The blueprint called for "deterministic packfile generation". But `git pack-objects`
output depends on git version, delta heuristics and threading, so byte-identical packs
across machines can't be guaranteed.

## Decision

Don't require determinism. The publisher creates a pack once, seeds it, and lists its
BitTorrent v2 infohash in the signed manifest. Other peers only ever re-share those exact
bytes. Integrity comes from the v2 per-piece SHA-256 merkle tree, and then from
`git index-pack`, which checks every object id.

## Consequences

- No fragile reproducibility requirement.
- Two publishers who pack the same history produce different infohashes. That is
  harmless because each manifest names its own packs.
