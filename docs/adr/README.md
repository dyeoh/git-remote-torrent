# Architecture Decision Records

Each ADR follows [MADR](https://adr.github.io/madr/). To add one, copy
[`template.md`](template.md) to `NNNN-short-title.md`. Once an ADR is accepted it is never
edited. A later decision supersedes it with a new ADR.

| # | Title | Status |
|---|-------|--------|
| [0001](0001-url-syntax.md) | URL syntax: `torrent::<url>` and `torrent://<pubkey>` | Accepted |
| [0002](0002-bep44-signed-manifests.md) | Signed ref manifests as BEP 44 mutable items | Accepted |
| [0003](0003-non-deterministic-packs.md) | Packs need not be deterministic | Accepted; v2 transport superseded by 0006 |
| [0004](0004-incremental-pack-chain.md) | Incremental pack chain with compaction | Accepted |
| [0005](0005-git-cliff-release-flow.md) | Release flow with git-cliff instead of release-plz | Accepted |
| [0006](0006-bittorrent-v1-and-bep44-value-layout.md) | BitTorrent v1 for packs; manifest signed as a byte string | Accepted |
