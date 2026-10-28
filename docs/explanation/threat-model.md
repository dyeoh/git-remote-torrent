# Threat model

This page covers git-remote-torrent 0.4: what it protects, from whom, what is already in
place, and what is still open. Report vulnerabilities as described in
[SECURITY.md](https://github.com/dyeoh/git-remote-torrent/blob/main/SECURITY.md).

## What is protected

- **The publisher's Ed25519 key.** It is the only authority over an address: whoever holds
  it decides which refs `torrent://<key>[/<name>]` points to.
- **The integrity of what cloners receive:** refs and objects must be exactly what the
  key signed.
- **Availability:** the repository stays cloneable when the forge, peers or mirrors go away.
- **Users' privacy:** IP addresses, and which repositories they fetch or serve.

## Who is involved

| Actor | Trusted for |
|---|---|
| The publisher | The content of their own address, nothing else |
| DHT nodes | Nothing. Any number of them may be hostile (Sybil and eclipse attacks) |
| BitTorrent peers | Nothing |
| HTTP mirrors | Nothing, including mirrors the manifest names |
| The upstream forge (hybrid mode) | Its refs, while it answers |
| Someone who stole the key | (an attacker) |
| Whoever hands you an address | (possibly an attacker; see H2) |
| Hosts on your LAN | Nothing |

**Trust anchor:** the key in `torrent://<key>`, or in `remote.<name>.torrentAddress`. Every
manifest, pack and ref is checked against it. Nothing on the network can change which key
a remote uses.

## Controls in place

- **Signatures.** Every manifest is checked with `verify_strict`, which rejects malleable
  signatures, including the cached copy read from disk. The signed value is strict
  canonical bencode with a nesting limit. Ref names must pass git's `check-ref-format`. The
  1000-byte limit is checked both when signing and when verifying.
- **Rollback.** A clone keeps the newest manifest it has fetched in `.git/torrent/fetched/`,
  and an older one from the DHT never replaces it. Publishing uses BEP 44 compare-and-swap.
  A seeder that loses it stops rather than fighting over the address, and a push from a
  second machine is refused.
- **Packs.** Each pack is a BEP 52 hybrid torrent. librqbit checks the SHA-1 of every piece,
  and the metadata is bound to the infohash. After download, both infohashes (SHA-1 and
  SHA-256) are derived again from the file and compared with the signed manifest
  ([ADR-0008](../adr/0008-hybrid-torrents-and-sha256.md)). Then git runs
  `index-pack --strict` (fsck on every object). Every wanted ref must then be present, and
  git runs its own connectivity check.
- **Mirrors.** Mirrors get no more trust than peers: the same two hashes, and a 4 GiB cap
  per pack.
- **Argument injection.** Upstream URLs starting with `-` are rejected. Push and fetch
  arguments must be `HEAD` or `refs/…`.
- **Key storage.** The path of the signing key is read only from **global** git config, so
  a cloned repository's config cannot point it elsewhere. Generated keys are written with
  mode 0600. Encrypted OpenSSH keys work and are prompted for.
- **Supply chain.** cargo-deny runs in CI. Releases ship `SHA256SUMS`, a cosign signature
  and build-provenance attestations. The GitHub Action checks the checksum and the
  attestation before the key is written to disk.

## Findings

Ranked by impact. Each fix is a separate change; the status column tracks them.

| | Finding | Status |
|---|---|---|
| H1 | Key compromise is permanent | Open: design in ADR-0010 |
| H2 | The trust store is not consulted when fetching | Fixed in 0.4 |
| M1 | Manifest mirrors make cloners send requests to any host | Fixed in 0.4 |
| M2 | A plain `sign` publishes every local branch and tag | Fixed in 0.4 |
| M3 | Fresh clones can be served a stale manifest | Mitigated in 0.4 |
| M4 | Generated keys are unencrypted; secrets stay in memory | Fixed in 0.4 |
| L1 | Pack size from peers is not capped | Open |
| L2 | Privacy: IPs, interests and LAN multicast | Open: documented below |
| L3 | DHT bootstrap is the easiest point for an eclipse | Documented below |
| L4 | CI publishing | Addressed by the action's design |

### H1. Key compromise is permanent

There is no rotation or revocation. Whoever holds the key can sign any refs at the address.
They can also publish `seq = i64::MAX`, after which the real publisher can never publish
there again, because clients and DHT nodes keep the highest sequence number. Nor can an
address say "this repository moved to another key".

**Until then:** use a key for one address (or one CI job) only, never an SSH key reused
everywhere, and encrypt it unless it lives in a CI secret.

**Planned:** [ADR-0010](../adr/0010-key-succession.md). Each manifest commits to a
successor key that is kept offline. Clients remember the first commitment they see, and
follow the address to that key when it publishes a manifest naming the old one. A thief
holding only the current key can't sign as the successor, and clients that have moved
ignore the old address, even at `seq = i64::MAX`. Fresh clones made after a theft can
fall back on keys in the trust store, including keys imported from the forge.

### H2. The trust store is not consulted when fetching

`git torrent trust` maintains `.git/torrent/trusted-keys`, but the helper never reads it. In
hybrid mode, any configured `torrentAddress` is taken on faith. Suppose a README says
`git clone -c remote.origin.torrentAddress=<attacker's key> torrent::https://forge/…`. While
the forge answers, git's object hashes keep the content honest. Once the forge is
unreachable, the attacker's manifest supplies the refs and therefore the code. The
manifest's signed upstream URL proves nothing, because anyone can sign anyone's URL.

**Fixed:** whenever refs come from a manifest (a `torrent://` remote, or a hybrid remote
whose upstream is gone), the helper prints the publisher's key, as an OpenSSH line and as
an address. If the repository's trust store has entries, other keys are refused.
Otherwise, a hybrid remote's key is pinned for its upstream in `.git/torrent/pins` on the
first fetch that succeeds against the live upstream, and a different key is refused later.
`git torrent trust add` accepts a new key and moves the pin to it.

What remains is trust on first use, as with SSH: a clone made with an attacker's
`torrentAddress` pins the attacker's key. The printed key, and keys imported beforehand
with `git torrent trust import`, are the defence.

### M1. Manifest mirrors make cloners send requests to any host

A manifest's mirror URLs make every cloner send GET requests to hosts the publisher
chooses. These can include `169.254.169.254` (cloud metadata), `localhost` and LAN
services. Redirects are followed, and plain `http://` mirrors show the network which
repositories you fetch. Responses are hash-checked, so the attacker can't inject content;
this is a blind probe and a side channel.

**Fixed:** mirrors named by a manifest connect through a resolver that keeps only public
addresses. It refuses loopback, private, link-local, carrier-grade NAT, unique-local,
multicast, unspecified and other special-purpose addresses, including IPv4 embedded in IPv6.
ureq resolves again for every connection, so redirects are checked too. Mirrors you
configure yourself (`torrent.mirror`, or `--mirror` for `adopt`) and the mirrors of your
own manifest (`adopt`, `verify-mirror`) are unrestricted. `torrent.allowPrivateMirrors=true`
lifts the restriction for a repository whose manifest names an internal mirror. Fetches
through a configured HTTP proxy are resolved by the proxy, which has to enforce its own
policy. Plain `http://` mirrors still show the network which packs you fetch.

### M2. A plain `sign` publishes every local branch and tag

Without `--update`, `git torrent sign` and `seed` publish all of `refs/heads` and
`refs/tags`, including private work branches. A published pack cannot be recalled: anyone
may have seeded it. `git push` and the GitHub Action already publish only the refs given.

**Fixed:** a plain `sign` (and `seed`) publishes the previous manifest's refs, the current
branch, branches a remote has, and tags on those. It lists what it publishes and names
what it leaves out. `--all` publishes everything.

### M3. Fresh clones can be served a stale manifest

Hostile DHT nodes, or an eclipse of a new node, can hide the newest manifest and serve an
older one that is still validly signed, or none at all. Anyone with an old copy can also
store it again once the newer one has expired (`republish --manifest`). Existing clones
are protected by their cached manifest. A fresh clone is not, and the manifest has no time
to judge freshness by.

**Mitigated:** manifests carry a signed time of signing, `t` (an optional field of format
1, so older clients ignore it and older manifests still verify). Whenever refs come from
a manifest, the helper prints its age. When the upstream is unavailable and the manifest
is more than 30 days old, it warns that a newer one may exist. `git torrent status` shows
the time as well.

This narrows the attack rather than closing it: an attacker can still serve a manifest
from last week, and a repository that is simply quiet looks the same as a stale one. Only
reaching an honest node, or the upstream, gives the newest manifest.

### M4. Generated keys are unencrypted, and secrets stay in memory

`git torrent key generate` always writes an unencrypted key. The seed and the PEM text
are not wiped from memory after use.

**Fixed:** `git torrent key generate --passphrase` writes an encrypted OpenSSH key (bcrypt
KDF, AES-256-CTR, as `ssh-keygen` does), and `sign` asks for the passphrase on the
terminal. The seed, the PEM text and the passphrase are held in buffers zeroed on drop,
and the signing key zeroes itself. This is best effort: copies the compiler makes on the
stack, or the operating system swaps out, are beyond its reach. CI keeps an unencrypted
key in a secret (see L4).

### L1. Pack size from peers is not capped

Mirror downloads stop at 4 GiB, but a pack from peers is as large as its signed torrent
says. Abusing it needs the publisher's key, since the infohash is signed.
**Fix:** apply the same cap once the torrent metadata arrives.

### L2. Privacy

- DHT lookups and announces reveal your IP address and which addresses and packs you are
  interested in, to whichever nodes you contact. The same is true of every BitTorrent client.
- Seeders accept connections on all interfaces (`0.0.0.0`) and announce what they hold.
  UPnP port forwarding is opt-in (`--upnp`).
- When DHT discovery is on, local service discovery (BEP 14) multicasts the infohashes
  being fetched or seeded on your LAN.
- Version 1 is for **public repositories**: anything signed into a manifest is public.

**Fix:** a `torrent.localDiscovery=false` setting. Use a private DHT
(`torrent.dhtBootstrap`) or mirrors only (`torrent.peerDht=false`) where this exposure is
unacceptable.

### L3. DHT bootstrap

The DHT is joined through mainline's well-known bootstrap nodes, and an eclipse attack is
easiest while a node's routing table is still empty. An eclipse can deny service or serve a
stale manifest (M3), but cannot forge one. Organisations can run their own bootstrap
nodes and set `torrent.dhtBootstrap`.

### L4. CI publishing

The [GitHub Action](../how-to/publish-from-ci.md) and its documentation:

- verify the binary's checksum and build provenance before the key is written to disk;
- write the key to `$RUNNER_TEMP` with mode 0600 and delete it when the step exits;
- document storing the key as an environment secret limited to the default branch and
  tags, never given to `pull_request` runs from forks or `pull_request_target`;
- recommend pinning the action to a reviewed tag or commit SHA;
- keep an untrusted copy of the manifest in the release. It is checked against the key
  before it is adopted or republished, and an older copy never replaces a newer manifest.

## Residual risks

These remain after every fix above, by design:

- **Availability is social.** A manifest lives on the DHT only while someone republishes
  it, and packs only while someone seeds or mirrors them.
- **A fresh clone of a forgotten address** can't tell a stale manifest from the newest one
  if every node it reaches is hostile. The timestamp (M3) narrows this; it does not close it.
- **SHA-1 git objects.** Packs are bound by SHA-256 as well, but refs name SHA-1 object
  ids, as in any SHA-1 git repository.
- **Dependencies.** librqbit, mainline and git itself are part of the attack surface.
