# URL syntax

| Form | Mode | Example |
|------|------|---------|
| `torrent::<git-url>` | Hybrid: the canonical upstream plus the swarm | `torrent::https://github.com/org/project.git` |
| `torrent://<ed25519-pubkey-hex>` | Pure P2P: the repo is identified by its publisher key | `torrent://3b6a27bc…` |

Hybrid mode uses git's built-in `<helper>::<address>` syntax. A form like
`torrent+https://` would make git look for a binary named `git-remote-torrent+https`,
which is why it isn't used. See [ADR-0001](../adr/0001-url-syntax.md).
