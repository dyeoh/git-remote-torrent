# 0011. Manifest format 1 frozen for 1.0

- Status: Accepted
- Date: 2026-10-29

## Context and problem

1.0 promises that the manifest format won't change incompatibly. Swarms outlive releases:
a manifest signed today may be read by a client built years from now, and a 1.0 client
may read manifests from versions not yet written. Before the promise is made, the
format has to be complete enough to grow without breaking it.

Three problems stood in the way:

- **The documented route for large ref sets didn't work.** The format reference said a
  `refs_root` indirection would come "later as a minor format change". Under "readers
  ignore unknown keys", a 1.0 reader would ignore it, see only the refs left inline, and
  treat the others as deleted. `git fetch --prune` would then remove them. Some future
  changes can't be ignored safely, and the format had no way to say so.
- **Some rules were looser than intended.** `packs2` was optional, so "every pack is
  checked against SHA-256" held only for chains signed since 0.4. `up` was any UTF-8
  string, which the helper prints and passes to git. Validation can't be tightened after
  the freeze, because that would reject manifests that used to be valid.
- **The format was described, not pinned down.** Only BEP 44's own vector tested the
  encoding. The manifest copy file had become an interchange format: the GitHub Action
  publishes it in releases, and `adopt` and `republish` read it. Yet `versioning.md`
  listed it as an internal cache.

This repository's own manifest #7 (6 refs, 7 packs) uses 834 of the 1000 bytes, so space
was measured rather than estimated.

## Considered options

1. **Freeze as it is,** and make any non-ignorable change a format version 2 with a
   transition period of signing both versions.
2. **Add a required-features list (`req`) and tighten the rules, then freeze.** Larger
   ref sets come later as a `req` feature.
3. **Also design and build ref-list indirection before 1.0,** so that 1.0 has no ref
   ceiling.

## Decision

Option 2.

- **`req`:** a list of feature names. A reader MUST reject a manifest that requires a
  feature it doesn't implement, and checks `req` before any other field except `v`. 1.0
  implements none. `refs-file` is reserved for ref-list indirection.
- **`packs2` is required** whenever there are packs.
- **`up` is validated:** at most 200 bytes, no whitespace or control characters, no
  leading `-`. It must be an https, http, ssh, git or file URL, an scp-style address,
  or an absolute path.
- **Writers can't sign what readers reject:** `grt-core` decodes a manifest before
  signing it.
- **`nk` and `prev` are reserved** for ADR-0010's key succession. Both are safe to ignore,
  so they can be added after 1.0 as an ordinary minor change.
- **Normative spec:** [manifest-format.md](../reference/manifest-format.md) becomes the
  specification. It covers the copy file, the pack chain, pack torrents and the mirror
  layout. Test vectors (manifests in `grt-core`, pack torrents cross-checked with
  libtorrent in `grt-swarm`) fail on any change to the format.

## Consequences

- 1.0 holds about 15 refs per manifest; the reference documents the limit. Ref-list
  indirection arrives in 1.x behind `req: ["refs-file"]`. 1.0 clients will then refuse
  those manifests and ask to be upgraded, instead of misreading them.
- Manifests signed by 0.3 (packs without `packs2`) are rejected, by clients and by
  `sign` itself, so a chain that 0.3 published and 0.4 never re-signed can't be
  continued. Its publisher starts a new chain under another `--name`. 0.4 re-signed every
  chain it touched with SHA-256, and no other 0.3 chains are known to be in use.
- A `remote.origin.url` that isn't a valid `up`, such as a relative path, is no longer
  recorded as the upstream. An invalid `--upstream` is an error.
- Changing a field rule, or how anything is signed or hashed, now takes format version 2
  with a transition period. Adding fields doesn't.
