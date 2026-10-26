# Roadmap

| Phase | Version | Deliverables | Exit criterion |
|-------|---------|--------------|----------------|
| 1. Protocol core | 0.1 | Helper handshake, delegate-to-upstream fetch, manifest signing, trust store, forge key import | `git clone torrent::https://github.com/…` works (done) |
| 2. Hybrid streaming | 0.2 | Pack seeding, BEP 44 manifests, swarm fetch, HTTP mirrors, `torrent://<key>` (done) | Kill-switch demo |
| 3. Read-write | 0.3 | Dual push, offline push and reconcile, CAS conflict handling, compaction, CI publish action | Push while the upstream is down, reconcile later |
| 4. Daemon & LAN | 0.4 | `git-torrentd` (seeding and republishing others' repositories), mDNS, push-webhook mirroring | LAN clone without internet |
| 5. Hardening | 1.0 | Threat model, key rotation, fuzzing, frozen format | Format frozen |
