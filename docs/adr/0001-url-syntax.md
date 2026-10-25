# 0001. URL syntax: `torrent::<url>` and `torrent://<pubkey>`

- Status: Accepted
- Date: 2026-10-25

## Context and problem

Git picks a remote helper from the URL. For `<scheme>://…` it runs
`git-remote-<scheme>`. For `<helper>::<address>` it runs `git-remote-<helper>` and
passes it `<address>`. The original blueprint proposed `torrent+https://…`, which would
make git look for a binary named `git-remote-torrent+https`.

## Considered options

1. `torrent+https://…`, installing one symlinked binary per scheme.
2. `torrent::https://…` (git's built-in syntax), plus `torrent://<key>` for pure P2P.

## Decision

Option 2. Hybrid remotes are written `torrent::<any git url>`, and pure P2P remotes are
written `torrent://<ed25519-pubkey-hex>`.

## Consequences

- One binary, no symlinks, and every upstream scheme (https, ssh, file) works for free.
- The P2P address is the publisher key, not an infohash, so it stays stable across pushes
  (see ADR-0002).
