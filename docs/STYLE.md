# Style Guide

This project follows [Google's Rust style guide](https://google.github.io/styleguide/rustguide.html).
Its Chromium- and Fuchsia-specific rules are dropped, and the project-specific rules below
are added. Tooling enforces whatever it can (`rustfmt.toml`, `[workspace.lints]` in
`Cargo.toml`, `clippy.toml`). Reviewers check the rest.

## Formatting

- Run `cargo +nightly fmt`. Nightly is needed for the import-grouping options. Lines are
  limited to 100 columns, and hex literals are lowercase.
- `#[rustfmt::skip]` is allowed only for deliberate alignment, such as protocol tables,
  or for generated code. Personal preference doesn't count.

## Naming

- Follow the std conventions: `as_` (cheap borrow), `to_` (expensive, borrows), `into_`
  (consumes). Getters are `foo()`/`foo_mut()`, setters are `set_foo()`.
- Constructors are `new`, `try_new` when fallible, and `from_x` paired with a `From` impl.
- Extension traits end in `Ext`.
- Don't invent abbreviations: `manifest`, not `mfst`. Domain acronyms that are already
  established (`dht`, `bep`, `oid`) are fine.
- Domain values are **newtypes**, not bare primitives: `InfoHash`, `PublicKey`, `RefName`,
  `ObjectId`, `Seq`. Mixing up a pubkey and an infohash should be a compile error.

## Errors

- Use `Result` for recoverable failures. Each library crate has one `thiserror` error
  enum, marked `#[non_exhaustive]`. Binaries use `anyhow` with `.context(..)`.
- `panic!` is only for violated invariants (bugs), never for bad input from the network.
  **All peer data is hostile.**
- Never use `unwrap()`; clippy denies it outside tests. Use `expect("why this cannot
  fail")`, or pattern matching instead of check-then-unwrap.
- Add `#[must_use]` to functions whose ignored result is likely a bug, such as builders
  and lazy values. `Result` is already `#[must_use]`. Clippy's blanket
  `must_use_candidate` lint is turned off because it fires on every getter.

## API design

- Use an `enum` instead of a `bool` argument whenever the call site would be unclear:
  `fetch(Mode::Hybrid)`, not `fetch(true)`.
- Accept `&str`, `&[T]` and `&Path`, never `&String`, `&Vec<T>` or `&PathBuf`.
- Prefer generics to `dyn Trait` in libraries.
- Implement std traits (`FromStr`, `Display`, `Default`, `From`) instead of custom methods
  that do the same thing.
- Add a trait only when there is a real need for one. The planned ones are the test seams
  `Upstream` and `Swarm`, so the helper can run against in-memory fakes.
- Public enums and public-field structs get `#[non_exhaustive]` unless the set of
  variants or fields is truly closed.
- Don't use `unsafe`; the workspace sets `unsafe_code = "forbid"`.

## Modules and imports

- Name each file after its primary item (`manifest.rs` holds `Manifest`).
- `lib.rs` and other non-leaf modules contain only `mod` declarations, re-exports and
  crate docs.
- Each `use` imports from exactly one module. Don't use glob imports, except
  `use super::*;` inside `#[cfg(test)] mod tests`.
- Call free functions through their module (`git::rev_list(..)`), but import types
  directly.

## Stdout is sacred

`git-remote-torrent` talks to git over stdout, so one stray `println!` breaks the
protocol. `print_stdout` is denied workspace-wide. Only the helper's protocol writer
module may `#[allow]` it. All diagnostics go through `tracing` to stderr.

## Async and concurrency

- `grt-core` and `grt-git` are synchronous. Tokio is used only in `grt-swarm`,
  `grt-upstream`, `grt-helper` and `grt-daemon`.
- Borrow first. Use `Arc` only to share across task boundaries. Clone only when a deep
  copy is actually needed.
- Never recover a poisoned mutex.

## Readability

- Add type annotations to `.parse()`, `.into()` and `.collect()` when the target type
  isn't obvious.
- Prefer a `for` loop to `.for_each()`. Bind long iterator chains to a named variable
  before looping.
- Use `format!` when joining more than two strings.

## Documentation and comments

- Every public item gets a `///` doc comment, enforced by `missing_docs = "deny"`. Include
  `# Errors` and `# Panics` sections where they apply, plus an example when usage isn't
  obvious.
- `//` comments explain *why*. If a comment explains *what*, rename things until it
  isn't needed.
- Write docs directly; don't generate them from macros.

## Tests

- Unit tests go in an inline `#[cfg(test)] mod tests`. Cross-crate and end-to-end tests
  go in `tests/`.
- Every `assert!` gets a message saying why the condition should hold.
- Protocol transcripts use snapshot tests (`insta`).
- Tests must not touch the real DHT or network. Use `mainline::Testnet` and local bare
  repos.
