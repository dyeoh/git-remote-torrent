# 0002. Signed ref manifests as BEP 44 mutable items

- Status: Accepted
- Date: 2026-10-25

## Context and problem

Refs change, but torrent infohashes are immutable. Peers need to find the *current* refs
for a repository and verify who published them, without trusting any server.

## Considered options

1. A custom signed manifest format, distributed over a custom gossip protocol.
2. BEP 44 mutable items: Ed25519-signed values, stored in the Mainline DHT under the
   public key, with a monotonic `seq` and compare-and-swap. BEP 46 already uses them to
   point at updatable torrents.

## Decision

Option 2. The manifest is a bencoded dictionary stored as a BEP 44 mutable item. If the
ref list doesn't fit in BEP 44's 1000-byte value limit, `refs_root` points to a small
torrent that holds the full list.

## Consequences

- No new cryptography to design. Existing DHT nodes store manifests, and every peer can
  verify them.
- `seq` gives rollback protection, and `cas` gives optimistic concurrency for pushes.
- Republishing is required: DHT items expire after about 2 hours, so seeders must
  re-put them.
