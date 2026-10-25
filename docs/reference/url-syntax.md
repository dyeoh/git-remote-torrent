# URL syntax

| Form | Mode | Example |
|------|------|---------|
| `torrent::<git-url>` | Hybrid: the canonical upstream plus the swarm | `torrent::https://github.com/org/project.git` |
| `torrent://<ed25519-pubkey-hex>` | Pure P2P: the key's only repository | `torrent://3b6a27bc…` |
| `torrent://<ed25519-pubkey-hex>/<name>` | Pure P2P: a named repository under the key | `torrent://3b6a27bc…/project` |

- **Hybrid mode** uses git's built-in `<helper>::<address>` syntax. The `<git-url>` can be
  anything git understands: `https://`, `ssh://`, scp-style `git@host:path` or `file://`.
  A form like `torrent+https://` would make git look for a binary named
  `git-remote-torrent+https`, so it isn't used. See [ADR-0001](../adr/0001-url-syntax.md).
- **Pure P2P mode** addresses a repository by its publisher key, which stays the same
  across pushes. The optional `<name>` (up to 64 bytes) becomes the BEP 44 salt, so one
  key can publish several repositories. See the
  [manifest format](manifest-format.md#signature).

## Rejected URLs

The upstream URL is passed to `git` as an argument, so the helper rejects:

- URLs starting with `-`, which git would read as an option
  (the CVE-2017-1000117 class of bug)
- `torrent::torrent::…`, which nests the helper inside itself
- `torrent://` URLs whose key isn't 64 hex digits, or whose name is longer than 64 bytes

## Current support

| Mode | Status |
|------|--------|
| Hybrid | `clone`, `fetch` and `push` work by passing everything through to the upstream. The swarm comes in 0.2. |
| Pure P2P | Not supported yet (0.2) |
