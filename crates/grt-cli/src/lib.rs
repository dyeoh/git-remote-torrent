//! Implementation of `git torrent`: publisher keys, the trust store, signed
//! manifests and seeding.
//!
//! This is an application crate, so it reports errors with `anyhow`. All
//! output goes through the `out` writer handed to [`run`] so it can be tested.

mod cli;
mod export;
mod key;
mod seed;
mod sign;
mod status;
mod trust;

use std::io::Write;
use std::path::Path;

pub use crate::cli::Cli;
use crate::cli::Command;

/// Runs one `git torrent` invocation from directory `cwd`.
///
/// # Errors
///
/// Returns an error describing what went wrong; the binary prints it.
pub fn run(cli: Cli, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    match cli.command {
        Command::Key(command) => key::run(command, out),
        Command::Trust(command) => trust::run(command, cwd, out),
        Command::Sign(args) => sign::run(&args, cwd, out),
        Command::Seed(args) => seed::run(&args, cwd, out),
        Command::Status => status::run(cwd, out),
        Command::Export(args) => export::run(&args, cwd, out),
    }
}
