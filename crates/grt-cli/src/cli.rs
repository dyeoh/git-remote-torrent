use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use grt_core::{ObjectId, RefName};

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
    /// Sign a manifest of this repository's branches and tags, extending the
    /// pack chain if they reach new objects.
    Sign(SignArgs),
    /// Sign, publish the manifest to the DHT, and seed its packs until Ctrl-C.
    Seed(SeedArgs),
    /// Show the signed manifest and compare it with the local refs.
    Status,
    /// Copy the signed manifest's packs into a directory as
    /// `<infohash>.pack`, ready to upload to an HTTP mirror.
    Export(ExportArgs),
    /// Replace the pack chain with one pack and sign the next manifest with
    /// the same refs. `sign` does this by itself when the manifest is full.
    Compact(CompactArgs),
    /// Push refs that went only to the swarm while the upstream was down to
    /// the upstream, now that it is back.
    Reconcile(ReconcileArgs),
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
    /// Base URL of an HTTP mirror serving `<infohash>.pack` files (see
    /// `git torrent export`). Repeat for several. Without it, the previous
    /// manifest's mirrors are kept.
    #[arg(long = "mirror", value_name = "URL")]
    pub(crate) mirrors: Vec<String>,
    /// Publish the previous manifest's refs with this one set, instead of
    /// all local branches and tags. Repeatable; used by `git push`.
    #[arg(long = "update", value_name = "REF=OID", value_parser = parse_update)]
    pub(crate) updates: Vec<(RefName, ObjectId)>,
    /// Publish the previous manifest's refs without this one. Repeatable.
    #[arg(long = "delete", value_name = "REF")]
    pub(crate) deletes: Vec<RefName>,
    /// Keep the previous manifest's refs unchanged (`git torrent compact`).
    #[arg(skip)]
    pub(crate) keep_refs: bool,
}

impl SignArgs {
    /// True if this signs the previous manifest's refs, changed only by
    /// `updates` and `deletes`, rather than the local refs.
    pub(crate) fn builds_on_previous(&self) -> bool {
        self.keep_refs || !self.updates.is_empty() || !self.deletes.is_empty()
    }
}

fn parse_update(value: &str) -> Result<(RefName, ObjectId), String> {
    let (name, id) = value
        .split_once('=')
        .ok_or_else(|| format!("expected REF=OID, got {value:?}"))?;
    Ok((
        name.parse().map_err(|error| format!("{error}"))?,
        id.parse().map_err(|error| format!("{error}"))?,
    ))
}

#[derive(Debug, Args)]
pub(crate) struct CompactArgs {
    /// Delete every stored pack that is not in the new chain. Peers part-way
    /// through fetching an old chain from this machine will have to start
    /// again.
    #[arg(long)]
    pub(crate) prune: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ReconcileArgs {
    /// The `torrent::<url>` remote whose upstream missed pushes.
    #[arg(default_value = "origin")]
    pub(crate) remote: String,
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// Directory to copy the packs into; created if missing.
    pub(crate) dir: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct SeedArgs {
    #[command(flatten)]
    pub(crate) sign: SignArgs,
    /// Address to accept peer connections on.
    #[arg(long, default_value = "0.0.0.0:0")]
    pub(crate) listen: SocketAddr,
    /// Join a private DHT through this node instead of the public Mainline
    /// DHT. Repeat for several nodes.
    #[arg(long = "dht-bootstrap", value_name = "HOST:PORT")]
    pub(crate) dht_bootstrap: Vec<String>,
    /// Don't announce packs on the BitTorrent DHT; peers must be given the
    /// listen address.
    #[arg(long)]
    pub(crate) no_peer_dht: bool,
    /// Ask your router to forward the listen port (UPnP), so peers outside
    /// your network can download packs. Changes the router's configuration.
    #[arg(long)]
    pub(crate) upnp: bool,
    /// Seconds between manifest republishes.
    #[arg(long, default_value_t = 1800, hide = true)]
    pub(crate) republish_secs: u64,
}
