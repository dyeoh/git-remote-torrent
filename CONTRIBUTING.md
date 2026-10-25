# Contributing

## Setup

```sh
rustup show                      # installs the pinned toolchain from rust-toolchain.toml
rustup toolchain install nightly --component rustfmt   # for the Google-style import formatting
cargo install cargo-nextest cargo-deny
```

## Checks (same as CI)

```sh
cargo +nightly fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace && cargo test --workspace --doc
cargo deny check
```

## Code style

Follow [docs/STYLE.md](docs/STYLE.md). It is based on Google's Rust style guide, and
lints in `Cargo.toml` enforce most of it.

## Commits and versioning

- Write commits as [Conventional Commits](https://www.conventionalcommits.org/):
  `feat(helper): add list command`, `fix(core): reject manifests with seq 0`.
  Mark breaking changes with `feat!:` or a `BREAKING CHANGE:` footer. CI lints PR commits.
- Don't edit versions or `CHANGELOG.md` by hand. `release-plz` opens a release PR that
  does both. See [docs/reference/versioning.md](docs/reference/versioning.md).
- Record significant design decisions as an ADR in [docs/adr/](docs/adr/).

## Pull requests

Keep each PR to one topic. Include tests, update docs when behaviour changes, and fill in
the PR template.
