# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).
## [1.0.0] - 2026-10-29

### Added
- Let cloners seed with `seed --follow`, and install seeders as services (#56)
- **BREAKING:** Freeze manifest format 1, with required features and test vectors (#58)
## [0.4.0] - 2026-10-28

### Added
- **BREAKING:** SHA-256 for every pack via hybrid v1/v2 torrents (#35)
- Git torrent republish keeps any repository findable (#38)
- Adopt an address on a new machine, and refuse shallow signs (#39)
- Verify-mirror and publish, for publishing without seeding (#40)
- Pure P2P as a first-class mode: seed many repos, republish others, service files (#42)
- Publish from GitHub Actions, with packs in a release (#43)
- Sign the time of signing into manifests, and warn about old ones (#50)

### Documentation
- **BREAKING:** Prior art in the README, scope (ADR-0009), two modes; remove the git-torrentd stub (#41)
- Threat model for keys, the DHT, peers and mirrors (#44)
- Key succession through a pre-committed successor key (#51)

### Fixed
- Retry DHT lookups that find nothing (#37)
- Mirrors named by a manifest may only reach public addresses (#45)
- Check the publisher key before serving refs from a manifest (#46)
- A plain sign publishes only refs that are already shared (#47)
- Encrypt generated keys with a passphrase, and wipe key material (#48)
- Cap pack size from peers, and let LAN multicast be turned off (#52)
- Count only vX.Y.Z tags as releases in git-cliff (#53)
- Say plainly where fetched packs came from (#54)
## [0.3.0] - 2026-10-26

### Added
- **BREAKING:** Push to the swarm (#30)
- **BREAKING:** Push while the upstream is down, reconcile later (#32)
- Compact the pack chain, by hand or when the manifest is full (#33)

### Fixed
- Refuse to publish an address from a second machine (#34)
## [0.2.0] - 2026-10-26

### Added
- **BREAKING:** V1 pack infohashes and BEP 44 byte-string signing (#19)
- Write and index packs for the pack chain (#20)
- Manifests on the DHT and packs over BitTorrent (#21)
- Git torrent seed and a pack chain behind every manifest (#22)
- **BREAKING:** Clone and fetch from the swarm (#27)
- Fall back to HTTP mirrors for packs, verified by infohash (#28)

### Changed
- Return typed refs from Repo (#17)

### Documentation
- Kill-switch tutorial, and fix finding seeders on the same network (#29)
## [0.1.0] - 2026-10-25

### Added
- Add signed ref manifests (#11)
- Pass-through torrent:: remote helper (#12)
- Git torrent key, trust, sign and status (#13)

### Documentation
- Explain benefits for centralised hosts (#9)
