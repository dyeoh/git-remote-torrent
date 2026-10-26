# 0009. Scope: a forge companion, not a P2P forge

- Status: Accepted
- Date: 2026-10-26

## Context and problem

A review of prior art ([prior-art.md](../explanation/prior-art.md)) found the following:

- GitTorrent (2015) built nearly the same design and became inactive for lack of users
  and seeders.
- Radicle is an active, funded P2P forge with collaboration features, seed nodes and its
  own network.
- Software Heritage already archives public code.

The roadmap's Phase 4 (a background daemon, mDNS on the LAN, webhook mirroring) would
rebuild what Radicle does.

## Considered options

1. **Narrow and finish:** make the project a companion to existing forges, centred on a
   zero-migration remote helper, signed manifests and verifiable mirrors. Finish CI
   publishing, the security work and a small 1.0.
2. **Wrap up as a portfolio prototype:** stop feature work now.
3. **Pivot to integration:** build on Radicle or Git LFS instead of our own transport.
4. **Continue the full roadmap,** daemon and LAN mesh included.

## Decision

Option 1. The project's distinctive parts are a zero-migration remote helper, verifiable
static mirrors and forge-key trust, and none of them depend on the daemon phase. Phase 4
is dropped. Interop with Radicle and Software Heritage is documented, not built.

## Consequences

- The roadmap's 1.0 requires: CI publishing (an action that signs, uploads to a mirror,
  verifies it and publishes), the threat model and its fixes, a frozen manifest format,
  and this repository publishing itself.
- The README describes durability honestly: a signed pointer plus a mirror, with peers
  as a bonus.
- Features that need a long-running process are out of scope, except `seed`, which stays
  a foreground command for people who want to seed.
