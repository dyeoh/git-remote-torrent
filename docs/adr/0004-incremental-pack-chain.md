# 0004. Incremental pack chain with compaction

- Status: Accepted
- Date: 2026-10-25

## Context and problem

Repacking the whole repository on every push makes every peer re-download everything.

## Decision

Each push publishes one self-contained (non-thin) pack, holding the objects reachable
from the new tips but not from the previous manifest's tips. The manifest lists packs in
order. `git torrent compact` replaces the chain with a single base pack once it grows
longer than a configurable limit.

## Consequences

- Fetches are incremental: a peer downloads only the packs it is missing.
- Non-thin packs give up some delta compression in exchange for independent verification
  of each pack.
- Compaction changes the infohash list, so it is a manifest update like any other push.
