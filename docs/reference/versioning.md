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

The rules live in the `[bump]` section of `cliff.toml`. The release flow
(`.github/workflows/release-pr.yml`) works like this:

1. **Every push to `main`** asks git-cliff for the next version. If that version isn't
   tagged yet, the workflow opens or updates a `chore(release): vX.Y.Z` PR. The PR sets
   the workspace version and the internal dependency versions, and regenerates
   `CHANGELOG.md` from the whole history. Its description is the new changelog section.
2. **Merging the release PR** tags `vX.Y.Z` on `main`. The `Release` workflow then builds
   binaries for four targets, records build provenance, signs `SHA256SUMS` with cosign
   and publishes a GitHub Release whose notes come from the changelog.
3. **If a release build fails** after the tag exists, fix `release.yml` on `main`, then
   rebuild that tag with `gh workflow run release.yml -f tag=vX.Y.Z`. Pull requests that
   change `release.yml` run the build matrix as a dry run first.
4. **Milestone releases** (0.2, 0.3, …) are started by hand. Run the workflow from the
   Actions tab with `version` set, for example `v0.2.0`.

The release PR is opened with the workflow's own token, so GitHub doesn't run CI on it.
It changes only version numbers and the changelog, which are checked again by the CI run
on `main` after the merge. See [ADR-0005](../adr/0005-git-cliff-release-flow.md).

## Planned milestones

| Version | Milestone |
|---------|-----------|
| 0.1 | Protocol core and trust |
| 0.2 | Hybrid streaming, DHT, kill-switch demo |
| 0.3 | Read-write sync |
| 0.4 | Daemon and LAN mesh |
| 1.0 | Manifest format frozen, hardened |
