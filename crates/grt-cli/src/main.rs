//! `git-torrent`: run as `git torrent <subcommand>`.

use std::io;

use clap::Parser;
use grt_cli::Cli;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    grt_cli::run(cli, &cwd, &mut io::stdout().lock())
}
