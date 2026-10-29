# Roadmap

| Phase | Version | Deliverables | Exit criterion |
|-------|---------|--------------|----------------|
| 1. Protocol core | 0.1 | Helper handshake, delegate-to-upstream fetch, manifest signing, trust store, forge key import | `git clone torrent::https://github.com/…` works (done) |
| 2. Hybrid streaming | 0.2 | Pack seeding, BEP 44 manifests, swarm fetch, HTTP mirrors, `torrent://<key>` (done) | Kill-switch demo |
| 3. Read-write | 0.3 | Dual push, offline push and reconcile, compaction, one-publisher conflict handling (done) | Push while the upstream is down, reconcile later |
| 4. Mirrors & CI | 0.4 | SHA-256 for every pack, `republish`, `adopt`, `verify-mirror`, `publish`, a GitHub Action that publishes from CI | This repository publishes itself from CI |
| 5. Hardening | 1.0 | Threat model and its fixes, key-succession design ([ADR-0010](../adr/0010-key-succession.md)), manifest format frozen with test vectors ([ADR-0011](../adr/0011-manifest-format-frozen.md)) | Format frozen |

A background daemon and LAN mesh were planned as Phase 4 and have been dropped: Radicle
already covers that ground. See [prior art](prior-art.md) and
[ADR-0009](../adr/0009-scope-forge-companion.md).
