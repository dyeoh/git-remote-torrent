# Threat model

> Placeholder. Filled in during phase 5.

Topics to cover: publisher key compromise and rotation, DHT Sybil/eclipse attacks,
malicious packs (`index-pack --strict`, `fsck`), hostile HTTP mirrors (packs are checked
against the signed SHA-1 and SHA-256 infohashes, ADR-0008, and capped at 4 GiB; mirror URLs make cloners send requests to
hosts the publisher chooses), manifest rollback (monotonic `seq`
pinning), and privacy (public repositories only in v1).
