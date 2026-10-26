# Prior art, and where this project fits

git-remote-torrent isn't the first attempt at peer-to-peer git. This page records what
else exists, what this project does differently, and what it deliberately leaves to other
tools. The scope that follows from it is recorded in
[ADR-0009](../adr/0009-scope-forge-companion.md).

## The landscape (researched October 2026)

| Project | What it does | Status | Relation to this project |
|---|---|---|---|
| [GitTorrent](https://github.com/cjb/GitTorrent) (2015) | Git over BitTorrent. Peers found through the mainline DHT, a BEP 44 mutable pointer signed by a key, packfiles shared as torrents, a `gittorrent://` remote helper | Inactive since 2015. Its bootstrap server has been offline since 2017 ([ctrl.blog, 2020](https://www.ctrl.blog/entry/git-p2p-compared.html)) | **The closest design.** This project is, in effect, GitTorrent revived and hardened: SHA-256 for packs, strict verification, rollback protection, mirrors, and a hybrid mode |
| [Radicle](https://radicle.dev/guides/protocol) (Heartwood) | Peer-to-peer code collaboration: Ed25519 node and repository identities, signed refs, seed nodes, gossip, issues and patches, `git-remote-rad` | Active. About 8,000 repositories and over 600 nodes seen weekly ([FAQ, April 2026](https://radicle.dev/faq)) | **The P2P forge.** It has its own network, identities and a node daemon. Anything collaborative or daemon-based belongs there, not here |
| [Software Heritage](https://ar5iv.labs.arxiv.org/html/1909.10760) | A universal, non-profit archive of public source code with full history, crawled from forges | Active, institutional | **Archival.** For "this code must never be lost", archiving there is the first step. It's a read-only archive, not a live remote |
| IPFS and Dat remotes (`git-remote-ipld`, igis, HyperGit) | Git over IPFS or Dat | Mostly inactive ([ctrl.blog, 2020](https://www.ctrl.blog/entry/git-p2p-compared.html)) | Same goal. They need a DHT lookup for each chunk, or rely on central trackers |
| Git LFS with P2P transfer agents (e.g. [radicle-lfs-transfer](https://git.hswro.org/mab122/radicle-lfs-transfer)) | Moves **large files** through Git LFS [custom or standalone transfer agents](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md), here over IPFS | Niche, early | **A different layer.** LFS carries big blobs referenced from commits; refs and ordinary git objects still travel over git. Not a competitor |
| Plain mirrors (a second forge, pull mirrors, self-hosting) | Copies on other hosts | Everywhere | Covers most "the forge took it down" cases with no new software, but nothing proves that a mirror is faithful |

The 2020 comparison concluded that P2P git tools fail through **adoption and
bootstrapping**, not through their technology. GitTorrent had the best overlay design and
still died without users or seeders.

## What this project adds

1. **No migration.** `torrent::https://github.com/…` sits in front of the forge you
   already use. Pull requests, CI and permissions stay where they are.
2. **No daemon to clone.** One helper binary finds manifests and packs through the
   mainline DHT that every BitTorrent client already uses. That works from a laptop and
   from GitHub-hosted runners.
3. **Verifiable "dumb" mirrors.** A signed pointer (BEP 44) plus content-addressed packs
   (SHA-1 and SHA-256, [ADR-0008](../adr/0008-hybrid-torrents-and-sha256.md)) can be
   served from any static host, including the forge's own release assets. Every copy is
   checked, so any mirror can be untrusted.
4. **Forge keys as the trust root.** `git torrent trust import <profile-url>` pins keys the
   developer already publishes, and they remain verifiable after the forge is gone.

## What it deliberately doesn't do

- **Collaboration** (issues, patches, multiple maintainers per repository): use Radicle.
- **A background daemon or LAN mesh:** Radicle seed nodes cover this. The roadmap no
  longer includes them.
- **Long-term archival:** also archive with Software Heritage. The manifest expires from
  the DHT about two hours after it was last published, and only a seeder, a mirror and
  republishing keep a repository reachable here.
- **Large files:** keep using Git LFS. A standalone transfer agent backed by the same
  manifest and hybrid torrents is possible, but nobody has asked for it.

## Honest limits

Availability is a social problem. After a takedown, a repository survives here only if
someone keeps the manifest published (`git torrent republish`, which needs no key) and
someone serves the packs (a seeder or a mirror). In practice, durability comes from
**a signed pointer plus at least one static mirror**, with peers as a bonus. Describe
the project that way, not as unstoppable.
