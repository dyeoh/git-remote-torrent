# 0005. Release flow with git-cliff instead of release-plz

- Status: Accepted
- Date: 2026-10-25

## Context and problem

The project releases all of its crates together: one shared version, one `vX.Y.Z` tag,
one root `CHANGELOG.md`, and binaries attached to a GitHub Release. The crates aren't
published to crates.io.

release-plz, chosen during scaffolding, is built around releasing each crate separately.
Getting it to behave as one release took several workarounds:

- **Version tracking.** With `publish = false`, it still checks crates.io to find the
  last release, so it needs `git_only` mode to read git tags instead.
- **Tags.** By default it creates one tag per crate. Our first run pushed seven
  `grt-*-v0.0.0` tags, which had to be deleted.
- **One shared version.** That needs a `version_group` entry for every crate.
- **One changelog.** A single changelog relies on `changelog_include`. In release-plz
  0.4.1 (`release_plz_core` 0.42.1, `command/update/updater.rs`), included commits are
  only merged when the crate already has a previous release. A first release therefore
  drops every commit that touches only other crates. A dry run of 0.1.0 left out #11.
- **Starting the release build.** Tags pushed with `GITHUB_TOKEN` don't start other
  workflows, so the release build has to be called directly whichever tool is used.

## Considered options

1. Keep release-plz with the workarounds above, and fix the first changelog by hand.
2. A short workflow built on git-cliff, which already formats the changelog.
3. `cargo-release`, run from a workflow.

## Decision

Option 2. git-cliff works on the whole repository's history, which matches our single
release stream:

- `git cliff --bumped-version` picks the next version using the rules in `cliff.toml`.
- `git cliff --tag vX.Y.Z` writes the changelog.
- A workflow of about 100 lines opens the release PR, tags it when merged, and calls
  `release.yml` directly.

## Consequences

- One tool, one tag, one changelog. Every rule is visible in `cliff.toml` and in
  `.github/workflows/release-pr.yml`.
- The version bump is our own code: two `sed` edits to the root `Cargo.toml`, tested
  against GNU sed. If the workspace manifest is restructured, those edits need updating.
- Milestone minor versions are set by hand with the workflow's `version` input. Pre-1.0
  rules only ever bump patch, or minor for breaking changes.
- If crates are published to crates.io later, add `cargo publish` to `release.yml`, or
  revisit release-plz.
