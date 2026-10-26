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

## How it compares

[GitTorrent](https://github.com/cjb/GitTorrent) tried git over BitTorrent in 2015;
[Radicle](https://radicle.dev) is the active peer-to-peer forge; Software Heritage archives
public code. This project stays a small companion to the forge you already use.
[Prior art](docs/explanation/prior-art.md) explains what it adds and what it leaves to
those projects.

## Documentation

- [Tutorial: your first immortal repo](docs/tutorials/first-immortal-repo.md)
- [How to set up keys and trust](docs/how-to/keys-and-trust.md) · [Mirror packs over HTTP](docs/how-to/http-mirrors.md)
- [Architecture](docs/explanation/architecture.md) · [Roadmap](docs/explanation/roadmap.md) · [ADRs](docs/adr/)
- [Contributing](CONTRIBUTING.md) · [Style guide](docs/STYLE.md) · [Security](SECURITY.md)

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
