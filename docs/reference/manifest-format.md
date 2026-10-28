# Manifest format (version 1)

A repository's published state is a **signed manifest**: a bencoded dictionary,
stored as a [BEP 44](https://www.bittorrent.org/beps/bep_0044.html) mutable item and
signed with the publisher's Ed25519 key. It is implemented in `grt-core`
(`Manifest`, `SignedManifest`).

## Manifest value

| Key | Type | Required | Meaning |
|-----|------|----------|---------|
| `v` | integer | yes | Format version. Currently `1`. |
| `refs` | dict: ref name → bytes | yes | Each ref's object id as raw bytes: 20 for SHA-1, 32 for SHA-256. All ids in one manifest use the same format. |
| `packs` | bytes | yes | Concatenated 20-byte v1 infohashes of each pack's hybrid torrent, oldest pack first. Empty if there are no packs. |
| `packs2` | bytes | no | Concatenated 32-byte v2 (SHA-256) infohashes of the **same** hybrid torrents, in the same order. Written since 0.4; readers verify every pack against it when present. See [ADR-0008](../adr/0008-hybrid-torrents-and-sha256.md). |
| `head` | bytes | no | The ref `HEAD` points to. It must be a key of `refs`. |
| `up` | bytes (UTF-8) | no | The canonical upstream URL. |
| `ws` | list of bytes (UTF-8) | no | Base URLs of HTTP mirrors serving each pack as `<base>/<infohash>.pack`. See [ADR-0007](../adr/0007-http-mirrors-instead-of-bep19.md). |
| `t` | integer | no | When the publisher signed the manifest, in seconds since the Unix epoch, by the publisher's clock. Written since 0.4; clients show a manifest's age and warn about old ones (see [Freshness](#freshness)). |

Keys are short because the encoded value, including its byte-string length prefix (see
[Signature](#signature)), must fit in **1000 bytes**, the BEP 44 limit. That allows
roughly 20 refs. Repositories with more refs will use an
indirection (`refs_root`), added later as a minor format change.

### Validation

Every manifest comes from an untrusted peer. Readers reject it when:

- the bencode isn't canonical (leading zeros, `-0`, unsorted or duplicate keys, trailing
  bytes, nesting deeper than 32 levels)
- `v` isn't `1`
- a ref name fails `git check-ref-format` or doesn't start with `refs/`
- an object id isn't 20 or 32 bytes, or SHA-1 and SHA-256 ids are mixed
- `packs` isn't a multiple of 20 bytes
- `packs2` is present but isn't a multiple of 32 bytes, or doesn't have one entry per
  pack in `packs`
- `head` names a ref that isn't in `refs`
- `t` is present but isn't a non-negative integer
- a `ws` entry isn't an `http://` or `https://` URL, is longer than 200 bytes, or
  contains whitespace

Readers **ignore unknown keys**, so later minor versions can add fields without breaking
older clients.

## Freshness

`seq` orders manifests, but it can't tell a new client whether the manifest it was given
is the newest. A hostile DHT node can serve an older one that is still validly signed.
`t` is covered by the signature, so it can't be changed without the key. When refs come
from a manifest, the helper prints its age (`note: manifest #12 was signed 3 days ago`).
When it does so because the upstream is unavailable and the manifest is more than 30 days
old, it also warns that a newer one may exist. A repository that hasn't changed in 30 days
draws the same warning: `t` records when refs were signed, not that the publisher is still
around. `git torrent status` shows the time too.

## Signature

The signature follows BEP 44. The signed message is the bencoded `salt` (only if the salt
isn't empty), `seq` and `v` entries, concatenated without the surrounding dictionary. The
manifest is carried in `v` as a bencode **byte string**:

```
4:salt<len>:<salt>3:seqi<seq>e1:v<len>:<manifest bytes>
```

This is the layout DHT implementations such as `mainline` use to store and verify values,
and it reproduces the BEP 44 test vectors exactly (`v` = `Hello World!` signs
`3:seqi1e1:v12:Hello World!`). Version 0.1.0 signed the manifest dictionary directly as
`v`, without the length prefix. Those signatures don't verify from 0.2.0 on. See
[ADR-0006](../adr/0006-bittorrent-v1-and-bep44-value-layout.md).

- **Key:** the publisher's Ed25519 key. Verification uses `verify_strict`, which rejects
  weak keys and malleable signatures.
- **Salt:** up to 64 bytes. One publisher key can host several repositories, each under
  its own salt. Leave it empty for a key's only repository.
- **Seq:** a 64-bit integer that must increase with every update. Peers keep the highest
  `seq` they have verified, which blocks rollback to older refs.

Verification happens **before** the manifest is parsed.

## Local cache encoding

The last verified manifest is cached as a bencoded dictionary:
`{k: pubkey, salt, seq, sig, v: <manifest bytes>}`. It is re-verified every time it is
loaded.

## Pack torrents

Each pack is described by a BEP 52 hybrid torrent, so anyone can recompute both of its
infohashes from the bytes:

- one file named `pack`, with `length` equal to the pack's size;
- a piece length of 256 KiB, doubled until there are at most 1024 pieces;
- an info dictionary with exactly these keys: `file tree`, `length`, `meta version` (2),
  `name`, `piece length`, `pieces`;
- **v1 infohash:** SHA-1 of the bencoded info dictionary;
- **v2 infohash:** SHA-256 of the same bytes.
