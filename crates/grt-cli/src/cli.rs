use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Keys, trust and signed manifests for git-remote-torrent.
#[derive(Debug, Parser)]
#[command(name = "git torrent", version)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Manage the key that signs your manifests.
    #[command(subcommand)]
    Key(KeyCommand),
    /// Manage which publishers this repository trusts.
    #[command(subcommand)]
    Trust(TrustCommand),
    /// Sign a manifest of this repository's branches and tags.
    Sign(SignArgs),
    /// Show the signed manifest and compare it with the local refs.
    Status,
}

#[derive(Debug, Subcommand)]
pub(crate) enum KeyCommand {
    /// Generate a new publisher key and make it your signing key.
    Generate {
        /// Overwrite an existing generated key.
        #[arg(long)]
        force: bool,
    },
    /// Sign with an existing OpenSSH Ed25519 key, such as `~/.ssh/id_ed25519`.
    ///
    /// Using the key you already publish on your forge lets others trust
    /// you with `git torrent trust import <profile-url>`.
    Use {
        /// Path to the private key file.
        path: PathBuf,
    },
    /// Print your public key and repository address.
    Show,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TrustCommand {
    /// Trust a publisher key.
    Add {
        /// An OpenSSH public key line (`ssh-ed25519 AAAA... comment`) or a
        /// 64-digit hex key.
        #[arg(required_unless_present = "own", num_args = 1..)]
        key: Vec<String>,
        /// A label for the key; defaults to the SSH key comment.
        #[arg(long)]
        label: Option<String>,
        /// Trust your own signing key.
        #[arg(long = "self", conflicts_with = "key")]
        own: bool,
    },
    /// Trust the Ed25519 keys a user publishes on their forge.
    Import {
        /// The user's profile URL, such as `https://github.com/alice`.
        profile: String,
    },
    /// List trusted keys.
    List,
    /// Stop trusting a key.
    Remove {
        /// The key, as hex or an OpenSSH public key line.
        #[arg(num_args = 1..)]
        key: Vec<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct SignArgs {
    /// The canonical upstream URL to record; defaults to `remote.origin.url`.
    #[arg(long)]
    pub(crate) upstream: Option<String>,
    /// Publish under `torrent://<key>/<name>` instead of `torrent://<key>`.
    #[arg(long, default_value = "")]
    pub(crate) name: String,
}
