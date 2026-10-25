# Versioning & compatibility

We follow [Semantic Versioning 2.0](https://semver.org/). All crates are released in
**lockstep**: they share one workspace version and one tag `vX.Y.Z`.

## What is public API

A change to any of these is breaking:

1. **CLI**: subcommands, flags and exit codes of `git-torrent` and `git-torrentd`.
2. **URL syntax** (see [url-syntax.md](url-syntax.md)).
3. **Manifest wire format**: has its own `v` field, versioned separately (below).
4. **Library crate APIs** (`grt-*`), checked by `cargo-semver-checks` in CI.

Log output, `--help` wording and internal on-disk cache layout under `.git/torrent/` are
**not** public API.

## Manifest format version

- Readers reject a manifest whose major `v` is unknown, and ignore keys they don't
  recognise.
- Adding an optional key is a minor format change. Changing or removing a key, or changing
  signature semantics, is a major change.
- Because swarms outlive releases, a format major bump requires an ADR and a dual-publish
  transition period.

## How versions are bumped

Versions come from [Conventional Commits](https://www.conventionalcommits.org/):

| Commit | Before 1.0 | From 1.0 |
|--------|-----------|-----------|
| `fix:` | patch | patch |
| `feat:` | patch | minor |
| `feat!:` / `BREAKING CHANGE:` | minor | major |

`release-plz` opens a release PR that bumps versions and writes `CHANGELOG.md`. Merging
it tags the release, and the `Release` workflow builds, signs and publishes the binaries.

## Planned milestones

| Version | Milestone |
|---------|-----------|
| 0.1 | Protocol core and trust |
| 0.2 | Hybrid streaming, DHT, kill-switch demo |
| 0.3 | Read-write sync |
| 0.4 | Daemon and LAN mesh |
| 1.0 | Manifest format frozen, hardened |
