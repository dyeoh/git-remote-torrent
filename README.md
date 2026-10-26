# git-remote-torrent

**Immortal code.** A Git remote helper that keeps repositories cloneable when the
central host goes away. Any HTTPS/SSH remote (GitHub, a self-hosted forge, a bare server) stays your canonical
upstream. Every push is also signed with Ed25519 and seeded to a BitTorrent swarm, so a
takedown, outage or revoked account doesn't take the code with it.

> **Status: pre-alpha (0.x).** `torrent::<url>` remotes work against any upstream, and
> clones and fetches from the swarm are landing for 0.2. See the
> [roadmap](docs/explanation/roadmap.md). While the version is 0.y.z, any minor release
> may contain breaking changes.

## Try it

```sh
cargo install --git https://github.com/dyeoh/git-remote-torrent grt-helper grt-cli
git clone torrent::https://github.com/octocat/Hello-World.git
cd Hello-World
git torrent key use ~/.ssh/id_ed25519   # sign with the key your forge already knows
git torrent seed                        # sign, publish to the DHT, seed until Ctrl-C
```

## Quickstart (planned)

```sh
cargo install git-remote-torrent                                  # installs git-remote-torrent, git-torrent, git-torrentd
git clone torrent::https://github.com/org/project.git             # hybrid: upstream + swarm
git clone torrent://<publisher-ed25519-key>                       # pure P2P, no server at all
```

Your git commands stay the same: `clone`, `fetch` and `push` work as usual.

## Why it helps centralised hosts too

git-remote-torrent doesn't replace your forge. The forge keeps its role as the canonical
upstream for pull requests, CI, permissions and the web UI. The swarm sits beside it and
takes over the jobs a single server handles worst:

- **Lower egress costs.** Clones and fetches come from peers first, and the host only
  covers what the swarm can't. A team on one office network shares objects over the LAN
  (mDNS), so big repos and fresh CI runners stop pulling the same packfiles across the
  internet again and again.
- **Absorbing demand spikes.** A trending repository or a big release gets faster as more
  people clone it, because every downloader becomes a seed. Today those spikes hit the host
  hardest.
- **Off-site redundancy for free.** Every seeder holds a verified replica. If the host has
  an outage, a region goes down or a storage incident happens, developers keep fetching
  and the history can be restored from the swarm.
- **The host as just one more seed.** A forge can serve packs over plain HTTP as a
  BitTorrent web seed (BEP 19). It stays useful as a fast, reliable source without being
  the single point of failure.
- **Trust that outlives the server.** Ref manifests are Ed25519-signed with keys that
  developers already publish to their forge. Peers can verify what the host says is
  canonical, and they can still verify it later if the host goes away.
- **Adoption without migration.** Developers' commands stay the same:
  `git clone torrent::https://…` is an opt-in prefix. A host can offer it to the users who
  want it, and their workflow doesn't change.

## Documentation

- [Tutorial: your first immortal repo](docs/tutorials/first-immortal-repo.md)
- [How to set up keys and trust](docs/how-to/keys-and-trust.md)
- [Architecture](docs/explanation/architecture.md) · [Roadmap](docs/explanation/roadmap.md) · [ADRs](docs/adr/)
- [Contributing](CONTRIBUTING.md) · [Style guide](docs/STYLE.md) · [Security](SECURITY.md)

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
