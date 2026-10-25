# git-remote-torrent

**Immortal code.** A Git remote helper that keeps repositories cloneable when the
central host goes away. Any HTTPS/SSH remote (GitHub, a self-hosted forge, a bare server) stays your canonical
upstream. Every push is also signed with Ed25519 and seeded to a BitTorrent swarm, so a
takedown, outage or revoked account doesn't take the code with it.

> **Status: pre-alpha (0.x).** Nothing works yet. See the [roadmap](docs/explanation/roadmap.md).
> While the version is 0.y.z, any minor release may contain breaking changes.

## Quickstart (planned)

```sh
cargo install git-remote-torrent                                  # installs git-remote-torrent, git-torrent, git-torrentd
git clone torrent::https://github.com/org/project.git             # hybrid: upstream + swarm
git clone torrent://<publisher-ed25519-key>                       # pure P2P, no server at all
```

Your git commands stay the same: `clone`, `fetch` and `push` work as usual.

## Documentation

- [Tutorial: your first immortal repo](docs/tutorials/first-immortal-repo.md)
- [Architecture](docs/explanation/architecture.md) · [Roadmap](docs/explanation/roadmap.md) · [ADRs](docs/adr/)
- [Contributing](CONTRIBUTING.md) · [Style guide](docs/STYLE.md) · [Security](SECURITY.md)

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
