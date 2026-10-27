# git-remote-torrent

**A companion for your forge.** A Git remote helper that keeps repositories cloneable when
the central host goes away. Any HTTPS/SSH remote (GitHub, a self-hosted forge, a bare
server) stays your canonical upstream. Each push is also published as an Ed25519-signed
manifest on the BitTorrent DHT, with its packs served by peers and by any static mirror and
verified with SHA-256. A takedown, outage or revoked account then doesn't take the code
with it, as long as someone keeps a mirror or a seeder.

> **Status: pre-alpha (0.x).** Clones and fetches work from the swarm, including after
> the upstream is gone. Pushes can publish to the swarm, even while the upstream is down,
> and `git torrent reconcile` catches the upstream up later. See the
> [roadmap](docs/explanation/roadmap.md). While the version is 0.y.z, any minor release
> may contain breaking changes.

## Try it

```sh
cargo install --git https://github.com/dyeoh/git-remote-torrent grt-helper grt-cli

# Publisher: sign your refs, seed your objects, publish to the DHT
cd your-project
git torrent key use ~/.ssh/id_ed25519      # or: git torrent key generate
git torrent seed                           # prints your torrent://<key> address

# Anyone, even after the upstream is gone:
git clone torrent://<key>                                           # pure P2P
git clone -c remote.origin.torrentAddress=torrent://<key> \
  torrent::https://github.com/you/project.git                      # upstream + swarm
```

The [kill-switch tutorial](docs/tutorials/first-immortal-repo.md) walks through it on one
machine in ten minutes.

Your git commands stay the same: `clone`, `fetch` and `push` work as usual.

## Why it helps centralised hosts too

git-remote-torrent doesn't replace your forge. The forge keeps its role as the canonical
upstream for pull requests, CI, permissions and the web UI. The swarm sits beside it and
takes over the jobs a single server handles worst:

- **Lower egress costs.** Clones and fetches come from peers and mirrors first, and the
  host only covers what they can't. Peers on the same network find each other
  automatically.
- **Absorbing demand spikes.** A trending repository or a big release gets faster as more
  people clone it, because every downloader becomes a seed. Today those spikes hit the host
  hardest.
- **Verifiable off-site copies.** Every seeder and mirror holds a replica that clients
  verify against the signed manifest. If the host has
  an outage, a region goes down or a storage incident happens, developers keep fetching
  and the history can be restored from the swarm.
- **The host as just one more seed.** A forge can serve packs over plain HTTP as a
  [mirror](docs/how-to/http-mirrors.md), checked against the signed manifest. It stays
  useful as a fast, reliable source without being the single point of failure.
- **Trust that outlives the server.** Ref manifests are Ed25519-signed with keys that
  developers already publish to their forge. Peers can verify what the host says is
  canonical, and they can still verify it later if the host goes away.
- **Adoption without migration.** Developers' commands stay the same:
  `git clone torrent::https://…` is an opt-in prefix. A host can offer it to the users who
  want it, and their workflow doesn't change.

## Prior art and how this compares

Peer-to-peer git has been tried before. This is what exists (researched October 2026), and
where this project fits. The longer version, with the reasoning behind the scope, is in
[docs/explanation/prior-art.md](docs/explanation/prior-art.md) and
[ADR-0009](docs/adr/0009-scope-forge-companion.md).

| Project | What it does | Status | Relation to this project |
|---|---|---|---|
| [GitTorrent](https://github.com/cjb/GitTorrent) (2015) | Git over BitTorrent: mainline DHT, a key-signed BEP 44 pointer, packfiles as torrents | Inactive since 2015; bootstrap server offline since 2017 ([ctrl.blog](https://www.ctrl.blog/entry/git-p2p-compared.html)) | **Nearly the same design.** This project is, in effect, GitTorrent revived and hardened |
| [Radicle](https://radicle.dev/guides/protocol) | P2P code collaboration: Ed25519 identities, signed refs, seed nodes, issues and patches | Active: about 8,000 repos and 600+ nodes weekly ([FAQ, April 2026](https://radicle.dev/faq)) | **The P2P forge.** P2P only, with its own network and node. Collaboration and daemons belong there |
| [Software Heritage](https://ar5iv.labs.arxiv.org/html/1909.10760) | A universal archive of public source code with full history | Active, institutional | **Archival.** Read-only; not a live remote |
| IPFS and Dat remotes (`git-remote-ipld`, igis, HyperGit) | Git over IPFS or Dat | Mostly inactive ([ctrl.blog](https://www.ctrl.blog/entry/git-p2p-compared.html)) | Same goal; a DHT lookup for each chunk, or central trackers |
| Git LFS P2P transfer agents (e.g. [radicle-lfs-transfer](https://git.hswro.org/mab122/radicle-lfs-transfer)) | Large files through LFS [custom or standalone agents](https://github.com/git-lfs/git-lfs/blob/main/docs/custom-transfers.md) | Niche, early | **A different layer:** LFS blobs only. Refs and git objects still travel over git |
| Plain mirrors (a second forge, pull mirrors, self-hosting) | Copies on other hosts | Everywhere | Covers most takedowns, but nothing proves a mirror is faithful |
| BitTorrent ([BEP 19](https://www.bittorrent.org/beps/bep_0019.html) web seeds) and IPFS (node or gateways) | Content delivery that runs central, P2P, or both | Mature | **Dual-mode precedents.** Their mutable pointers, though, stay with a central party or need your own node |

**What this project adds**
- **No migration.** `torrent::https://…` sits in front of the forge you already use.
- **No daemon to clone.** One helper binary on the mainline DHT. It works from a laptop and
  from GitHub-hosted runners.
- **Verifiable static mirrors.** A signed manifest plus packs checked with SHA-1 and
  SHA-256 ([ADR-0008](docs/adr/0008-hybrid-torrents-and-sha256.md)), so any host can be an
  untrusted mirror, including the forge's own release assets.
- **Authority in a key, not a host.** That is what lets the same repository run with a
  forge or with no server at all (below).

**What it leaves to others:** collaboration (Radicle), long-term archival (Software
Heritage, worth using as well), and large files (Git LFS).

**Honest limits.** P2P git tools have failed on adoption, not technology. GitTorrent had the
best design of its generation and still died without seeders. A repository stays reachable
here only while someone keeps its manifest published (`git torrent republish`, which needs
no key) and someone serves its packs (a seeder or a mirror). One machine publishes each
address; there is no multi-writer collaboration.

## Two ways to run it

| | Forge plus P2P | Pure P2P |
|---|---|---|
| Remote | `torrent::https://github.com/you/project.git` | `torrent://<key>[/<name>]` |
| Canonical source | The forge, while it's up | The publisher's signed manifest |
| Packs come from | Peers and mirrors first, the forge as fallback | Peers and mirrors |
| If the forge goes away | The swarm serves refs and packs; `reconcile` replays pushes when it returns | Nothing to lose |
| Needs running | Nothing new | An always-on `git torrent seed` (a service, not a daemon) or a mirror |

Set up the pure mode with [Run as pure P2P](docs/how-to/pure-p2p.md), which includes ready-made
systemd and launchd service files. With neither a seeder nor a server of your own, the
[GitHub Action](docs/how-to/publish-from-ci.md) publishes every push, with packs mirrored in
a release.

## Documentation

- [Tutorial: your first immortal repo](docs/tutorials/first-immortal-repo.md)
- [How to set up keys and trust](docs/how-to/keys-and-trust.md) · [Mirror packs over HTTP](docs/how-to/http-mirrors.md) · [Publish from CI](docs/how-to/publish-from-ci.md)
- [Architecture](docs/explanation/architecture.md) · [Roadmap](docs/explanation/roadmap.md) · [ADRs](docs/adr/)
- [Contributing](CONTRIBUTING.md) · [Style guide](docs/STYLE.md) · [Security](SECURITY.md)

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
