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

## Backing a hybrid remote with the swarm

A `torrent::<git-url>` remote uses the swarm only if it knows the publisher's address.
Set it per remote, for example while cloning:

```sh
git clone -c remote.origin.torrentAddress=torrent://<key>[/<name>] \
  torrent::https://github.com/org/project.git
```

Without `remote.<name>.torrentAddress`, a hybrid remote passes everything through to
the upstream.

## Configuration

The helper reads these keys from git config, so they can be set per repository, globally,
or for one command with `git -c`.

| Key | Default | Meaning |
|-----|---------|---------|
| `remote.<name>.torrentAddress` | unset | The `torrent://` address that backs a hybrid remote |
| `remote.<name>.torrentPublish` | `false` | Pushes to this remote sign a new manifest with your signing key. Set it only on remotes whose address is your key |
| `torrent.dhtBootstrap` | the public DHT | `host:port` of a DHT node to join instead; repeat for several |
| `torrent.peer` | none | `host:port` of a peer to ask for packs; repeat for several |
| `torrent.peerDht` | `true` | Find pack peers on the DHT. Turn off for LAN-only or test setups |
| `torrent.mirror` | none | Base URL of an HTTP mirror to try after peers, besides those the manifest lists; repeat for several |
| `torrent.fetchTimeout` | `120` | Seconds allowed for resolving the manifest, and for each pack. When mirrors are available, peers get at most 30 seconds per pack before the mirrors are tried |

## Current support

| Mode | `clone` / `fetch` | `push` |
|------|-------------------|--------|
| Hybrid | Refs from the upstream, objects from the swarm when the manifest covers them; the swarm serves both when the upstream is gone | To the upstream; with `torrentPublish`, what the upstream accepts is also signed into the manifest. If the upstream is unreachable, to the swarm only, until `git torrent reconcile` |
| Pure P2P | From the swarm | With `torrentPublish`, signed into the manifest. Non-fast-forward updates need a force push |
