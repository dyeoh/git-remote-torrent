//! Where a repository keeps packs and manifests, under `.git/torrent/`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use grt_core::{InfoHash, PublicKey};

use crate::hybrid::PACK_FILE;

/// `<torrent_dir>/packs`: one `<infohash>/pack` directory per stored pack.
pub fn packs_dir(torrent_dir: &Path) -> PathBuf {
    torrent_dir.join("packs")
}

/// Where the pack with infohash `pack` is stored, ready to be seeded. The
/// file is named `pack`, as its torrent is.
pub fn stored_pack_path(torrent_dir: &Path, pack: InfoHash) -> PathBuf {
    packs_dir(torrent_dir)
        .join(pack.to_string())
        .join(PACK_FILE)
}

/// Where the last manifest fetched from `torrent://<key>[/<salt>]` is kept:
/// `fetched/<hex key>[.<hex salt>]`.
pub fn fetched_manifest_path(torrent_dir: &Path, key: &PublicKey, salt: &[u8]) -> PathBuf {
    let mut name = key.to_string();
    if !salt.is_empty() {
        name.push('.');
        for byte in salt {
            let _ = write!(name, "{byte:02x}");
        }
    }
    torrent_dir.join("fetched").join(name)
}
