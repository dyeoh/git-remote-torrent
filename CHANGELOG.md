# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).
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
