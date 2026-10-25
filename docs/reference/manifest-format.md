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
| `packs` | bytes | yes | Concatenated 32-byte BitTorrent v2 infohashes, oldest pack first. Empty if there are no packs. |
| `head` | bytes | no | The ref `HEAD` points to. It must be a key of `refs`. |
| `up` | bytes (UTF-8) | no | The canonical upstream URL. |

Keys are short because the whole encoded value must fit in **1000 bytes**, the BEP 44
limit. That allows roughly 20 refs. Repositories with more refs will use an
indirection (`refs_root`), added later as a minor format change.

### Validation

Every manifest comes from an untrusted peer. Readers reject it when:

- the bencode isn't canonical (leading zeros, `-0`, unsorted or duplicate keys, trailing
  bytes, nesting deeper than 32 levels)
- `v` isn't `1`
- a ref name fails `git check-ref-format` or doesn't start with `refs/`
- an object id isn't 20 or 32 bytes, or SHA-1 and SHA-256 ids are mixed
- `packs` isn't a multiple of 32 bytes
- `head` names a ref that isn't in `refs`

Readers **ignore unknown keys**, so later minor versions can add fields without breaking
older clients.

## Signature

The signature follows BEP 44 exactly. The signed message is the bencoded `salt` (only if
the salt isn't empty), `seq` and `v` entries, concatenated without the surrounding
dictionary:

```
4:salt<len>:<salt>3:seqi<seq>e1:v<manifest bytes>
```

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
