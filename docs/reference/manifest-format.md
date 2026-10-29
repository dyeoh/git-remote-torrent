# Manifest format (version 1)

**Status: frozen as of 1.0** ([ADR-0011](../adr/0011-manifest-format-frozen.md)). This page
is the specification. It changes only in the ways [Evolution](#evolution) allows. The
words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

A repository's published state is a **signed manifest**: a bencoded dictionary, stored as
a [BEP 44](https://www.bittorrent.org/beps/bep_0044.html) mutable item and signed with the
publisher's Ed25519 key. Its packs are BitTorrent hybrid torrents that peers and HTTP
mirrors serve. `grt-core` (`Manifest`, `SignedManifest`) and `grt-swarm` implement it, and
[test vectors](#test-vectors) pin it down for other implementations.

## Manifest value

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `v` | integer | yes | Format version: `1` |
| `refs` | dict: ref name → bytes | yes | Each ref's object id as raw bytes: 20 for SHA-1, 32 for SHA-256 |
| `packs` | bytes | yes | Concatenated 20-byte v1 infohashes of the pack chain, oldest first; empty if there are no packs |
| `packs2` | bytes | if `packs` isn't empty | Concatenated 32-byte v2 (SHA-256) infohashes of the same packs, in the same order ([ADR-0008](../adr/0008-hybrid-torrents-and-sha256.md)) |
| `head` | bytes | no | The ref `HEAD` points to |
| `up` | bytes (UTF-8) | no | The canonical upstream: where the repository lives on a forge |
| `ws` | list of bytes (UTF-8) | no | Base URLs of [HTTP mirrors](#http-mirrors) ([ADR-0007](../adr/0007-http-mirrors-instead-of-bep19.md)) |
| `t` | integer | no | When the manifest was signed, in seconds since the Unix epoch, by the signer's clock ([Freshness](#freshness)) |
| `req` | list of bytes (UTF-8) | no | [Required features](#required-features) |

Field rules:

- **`refs`:** every name MUST start with `refs/` and pass `git check-ref-format`. All ids
  MUST have the same length: a manifest describes a SHA-1 or a SHA-256 repository, never
  both.
- **`head`:** MUST be a key of `refs`.
- **`up`:** at most 200 bytes, with no whitespace or control characters, and not starting
  with `-`. It MUST be an `https://`, `http://`, `ssh://`, `git://` or `file://` URL, an
  scp-style `[user@]host:path`, or an absolute path.
- **`ws`:** each entry MUST be an `http://` or `https://` URL of at most 200 bytes, with
  no whitespace or control characters.
- **`t`:** MUST NOT be negative.

### Writers

A writer MUST encode canonically (sorted keys, no leading zeros), MUST write `packs2`
whenever it writes packs, and SHOULD write `t`. It MUST NOT sign a manifest that the
[validation rules](#validation) would reject; `grt-core` decodes everything before
signing it.

### Size

The encoded value, together with its byte-string length prefix (see
[Signature](#signature)), MUST fit in **1000 bytes**, the BEP 44 limit. Each ref costs
about 42 bytes and each pack 52; the other fields cost about 250. One manifest therefore
holds about **15 refs**. A writer that runs out of room compacts the chain into a single
pack ([ADR-0004](../adr/0004-incremental-pack-chain.md)) before it gives up. Larger ref sets
are planned as a [required feature](#required-features) (`refs-file`).

## Required features

`req` lists features that change what the other fields mean, so a reader that ignores
them would misread the manifest. A reader MUST reject a manifest whose `req` names a
feature it does not implement, and MUST check `req` before using any other field except
`v`. An empty or missing `req` requires nothing.

Version 1.0 implements no features. Names already reserved:

| Feature | Meaning |
|---|---|
| `refs-file` | The ref list is too large for the manifest and is stored elsewhere; planned |

## Reserved keys

Writers MUST NOT use these keys for anything other than their planned meaning. Readers
ignore them until they implement them.

| Key | Planned meaning |
|---|---|
| `nk` | The committed successor key ([ADR-0010](../adr/0010-key-succession.md)) |
| `prev` | The key a successor address replaces ([ADR-0010](../adr/0010-key-succession.md)) |

## Validation

Every manifest comes from an untrusted peer. A reader MUST verify the
[signature](#signature) first, then MUST reject the manifest when:

- the bencode isn't canonical: leading zeros, `-0`, unsorted or duplicate keys, trailing
  bytes, or nesting deeper than 32 levels;
- `v` is missing or isn't `1`;
- `req` names a feature the reader doesn't implement;
- a required key is missing, or any known key has the wrong type;
- a field breaks the [field rules](#manifest-value);
- `packs` isn't a multiple of 20 bytes, or `packs2` isn't a multiple of 32 bytes or
  doesn't have one entry per pack.

A reader MUST ignore keys it doesn't recognise.

## Evolution

Swarms outlive releases, so format 1 changes only like this:

- **Adding an optional key that readers may safely ignore** is a minor change, such as
  `nk`. Older readers keep working and simply don't use it.
- **Adding a key that older readers must not ignore** requires a new feature name in
  `req`, which older readers then refuse. This is a minor change too, because it fails
  safely.
- **Changing or removing a key, changing a field rule, or changing how anything is signed
  or hashed** is a major change. It needs an ADR, `v: 2`, and a transition period in
  which publishers sign both versions.

Making validation stricter counts as a major change, because it rejects manifests that
were valid.

## Signature

The signature follows BEP 44. The signed message is the bencoded `salt` (only if the salt
isn't empty), `seq` and `v` entries, concatenated without the surrounding dictionary. The
manifest is carried in `v` as a bencode **byte string**:

```
4:salt<len>:<salt>3:seqi<seq>e1:v<len>:<manifest bytes>
```

This is the layout DHT implementations such as `mainline` use to store and verify values,
and it reproduces the BEP 44 test vectors exactly (`v` = `Hello World!` signs
`3:seqi1e1:v12:Hello World!`). See
[ADR-0006](../adr/0006-bittorrent-v1-and-bep44-value-layout.md).

- **Key:** the publisher's Ed25519 key. Verification MUST reject weak keys and malleable
  signatures (`verify_strict`).
- **Salt:** at most 64 bytes. One key can host several repositories, each under its own
  salt, which is the `<name>` in `torrent://<key>/<name>`.
- **Seq:** a 64-bit integer that MUST increase with every update. Readers MUST NOT replace
  a manifest with one of a lower `seq` for the same key and salt.

## Pack chain

`packs` is a chain ([ADR-0004](../adr/0004-incremental-pack-chain.md)):

- Readers MUST index packs **oldest first**. A pack's objects may refer to objects in
  earlier packs.
- Packs MUST NOT be thin: every delta's base is in the same pack. Readers index each pack
  with `git index-pack --strict`.
- Together, the chain MUST contain every object reachable from every ref in `refs`.
- Compaction replaces the whole chain with one pack.

## Pack torrents

Each pack is described by a BEP 52 **hybrid** torrent, so anyone can recompute both of
its infohashes from the bytes:

- one file named `pack`, whose `length` is the pack's size;
- a piece length of 256 KiB, doubled until there are at most 1024 pieces;
- an info dictionary with exactly these keys: `file tree`, `length`, `meta version` (2),
  `name` (`pack`), `piece length`, `pieces`;
- **v1 infohash:** SHA-1 of the bencoded info dictionary;
- **v2 infohash:** SHA-256 of the same bytes.

A reader MUST check every downloaded pack against both of its infohashes from the
manifest, whatever the source, before indexing it.

## HTTP mirrors

A mirror with base URL `<base>` serves each pack at `<base>/<v1 infohash>.pack`, with the
infohash in lowercase hex and any trailing `/` on the base removed. Mirrors are untrusted;
their packs are checked like peers' packs. A reader SHOULD refuse to connect to non-public
addresses for mirrors that the manifest names, unless its user allows it (threat model M1).

## Manifest copy file

A signed manifest is stored and exchanged as one bencoded dictionary:

| Key | Value |
|---|---|
| `k` | The 32-byte public key |
| `salt` | The salt, possibly empty |
| `seq` | The sequence number |
| `sig` | The 64-byte signature |
| `v` | The manifest value bytes |

It is the cache under `.git/torrent/`, the `manifest` asset the
[GitHub Action](../how-to/publish-from-ci.md) keeps in a release, and the input to
`git torrent adopt --manifest` and `republish --manifest`. A reader MUST verify it like a
manifest from the DHT.

## Freshness

`seq` orders manifests, but it can't tell a new client whether the manifest it was given
is the newest. A hostile DHT node can serve an older one that is still validly signed.
`t` is covered by the signature, so it can't be changed without the key. When refs come
from a manifest, the helper prints its age (`note: manifest #12 was signed 3 days ago`).
When it does so because the upstream is unavailable and the manifest is more than 30 days
old, it also warns that a newer one may exist. A repository that hasn't changed in 30 days
draws the same warning: `t` records when refs were signed, not that the publisher is still
around. `git torrent status` shows the time too.

## Test vectors

- **Manifests:**
  [`crates/grt-core/tests/vectors/manifest-v1.txt`](https://github.com/dyeoh/git-remote-torrent/blob/main/crates/grt-core/tests/vectors/manifest-v1.txt)
  - **Valid manifests:** the empty manifest, every field, and a SHA-256 repository. Each
    gives its key seed, salt, `seq`, exact value bytes, signature and copy file.
  - **Invalid manifests:** each comes with the reason it's rejected, covering every rule
    above.
- **Pack torrents:** the v1 and v2 infohashes of three deterministic packs, which
  libtorrent 2.1.1 reproduces, are in `matches_libtorrent` in
  [`crates/grt-swarm/src/hybrid.rs`](https://github.com/dyeoh/git-remote-torrent/blob/main/crates/grt-swarm/src/hybrid.rs).

A change that makes either of them fail changes the format.
