# 0010. Key succession: a pre-committed successor key

- Status: Proposed
- Date: 2026-10-27

## Context and problem

An address is a key: `torrent://<key>[/<name>]` resolves to the BEP 44 item that key signs.
Nothing else has authority over it. This gives the threat model's H1
([threat model](../explanation/threat-model.md#h1-key-compromise-is-permanent)) two parts:

- **No rotation.** A publisher who wants a new key, because the old one is too widely
  exposed or its algorithm ages, can't tell existing clones where to go.
- **No recovery from theft.** Whoever steals the key can sign any refs at the address.
  They can also publish `seq = i64::MAX`, after which clients and DHT nodes refuse
  anything the real publisher signs there, since BEP 44 keeps the highest `seq`.

So whatever announces a new key can't depend on the old key alone, because a thief holds
it too. And it can't depend on the old address alone, because a thief can take that over
for good. The announcement needs an anchor the stolen key can't forge, and it has to be
found somewhere other than the old address.

Constraints:

- A manifest must fit in 1000 bytes, about 20 refs. Every byte spent on keys is taken
  from refs.
- BEP 44 items have exactly one signing key. Threshold or multi-signature schemes would
  need a different DHT record type that no DHT node stores.
- Clients have no process running between fetches ([ADR-0009](0009-scope-forge-companion.md)),
  so the client can only check for succession when git runs it.
- Hybrid remotes already have an independent anchor: the forge. A publisher's forge
  account lists their keys, and `git torrent trust import <profile>` reads them.

## Considered options

1. **A revocation or "moved to Y" record signed by the old key.** The simplest option,
   but a thief can sign "moved to Z" just as well. It helps with planned rotation only.
2. **A pre-committed successor key** (as in KERI's pre-rotation). Each manifest names
   the next key, `nk`, whose private half is kept offline and never used until it's
   needed. Clients remember the first `nk` they see for an address. To rotate, the
   publisher signs with that key at its own address, and a client that remembers `nk`
   follows. A thief holding only the current key can't sign as `nk`.
3. **The forge as the only anchor:** a client follows to any key that the upstream
   account lists. This works only for hybrid remotes while the forge answers, and it
   trusts the forge account, which may be what failed.
4. **A transparency log or registry** (Sigstore-style, or a key server). Strong, but it
   brings back a central service that pure P2P is meant to do without.
5. **Expiring keys:** addresses valid only until a date. This limits the damage but
   forces every user to move periodically, and a client can't trust its own clock against
   a hostile network.

## Decision

Option 2, with the forge (option 3) and the trust store as further anchors for clients
that saw no commitment.

**Commitment.** Manifest format 1 gains an optional field `nk`: the 32-byte successor
public key. `git torrent key generate --successor` writes a second, encrypted key, which
should then be stored offline. From then on, `sign` includes its public half in every
manifest. That costs 37 bytes, about one ref.

**First seen wins.** A client records the first `nk` it verifies for an address in
`.git/torrent/successors`, next to the pins of [H2](../explanation/threat-model.md#h2-the-trust-store-is-not-consulted-when-fetching).
A later manifest at the address that drops `nk`, or names a different one, is refused,
with the same "if this is really the publisher…" message as a changed pin. A thief who
signs a manifest naming their own successor therefore gets nothing from clients that had
already fetched.

**Succession.** The publisher signs the first manifest at the successor's address with
two extra fields: `prev`, the old key, and its own fresh `nk`. When a client resolves the
old address, it also looks up the successor's address. That lookup runs whenever it has
an `nk` for the address, at most once per fetch, and concurrently with the main one.
If the successor's address has a manifest whose `prev` is the old key, the client:

1. prints the change (`note: <old> has moved to torrent://<new>`);
2. records the new key as the address's successor, and as the pin for its upstream;
3. serves refs from the new address from then on. For a `torrent://` remote, it suggests
   `git remote set-url` and keeps working without it.

Then whatever the thief publishes at the old address, `seq = i64::MAX` included, is
ignored by these clients.

**Clients without a commitment** include fresh clones made after a theft, and clones of
addresses that never set `nk`. They follow only to a key their trust store holds. For a
hybrid remote, that includes one that `git torrent trust import` read from the upstream
account. Otherwise they see the old address as it is, and the printed key is their only
defence, as with H2's trust on first use.

**Planned rotation** is the same process without the theft: sign at the successor
address, then (optionally) publish a last manifest at the old address naming the
successor, for clients that never saw `nk`.

The format change is additive: `nk` and `prev` are unknown keys to older clients, which
ignore them.

## Consequences

- Recovery from theft works for every client that fetched before the theft, as long as
  the successor key stays offline and unused. It doesn't help fresh clones that only ever
  saw the thief's manifests. For those, the forge and published keys are the anchor.
- Publishers take on a second key to keep safe. Losing it leaves the address with no
  recovery, as today. Anyone who doesn't set one changes nothing.
- Clients do one more DHT lookup per fetch, for addresses that set `nk`.
- `nk` takes about one ref's worth of manifest space.
- Successions can chain: each successor commits to the next.
- The threat model's H1 moves from "open" to "mitigated" once this is implemented.
  Until then it stays open, with the advice it gives now: one key per address, encrypted
  unless it lives in a CI secret.
- Not covered: a thief who steals the current key *and* the successor key, and a
  publisher who wants several people to control the address. The latter needs a
  multi-writer record that BEP 44 can't express, and is out of scope
  ([ADR-0009](0009-scope-forge-companion.md)).
