use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use grt_core::{InfoHash, InfoHashV2};

use crate::error::Error;
use crate::pack_swarm::verify_pack;

/// Largest pack accepted from a mirror, so a hostile one cannot fill the disk.
pub const MAX_MIRROR_PACK_LEN: u64 = 4 << 30;

/// Downloads the pack with infohash `pack` from the HTTP mirror `base`
/// (`<base>/<infohash>.pack`) into its own directory under `out_dir`, and
/// returns the path of the pack file.
///
/// The file is accepted only if its hybrid torrent has infohash `pack` and,
/// when the manifest lists one, SHA-256 infohash `pack_v2`, so mirrors are as
/// untrusted as peers.
///
/// # Errors
///
/// Returns [`Error::Mirror`] if the request fails, takes longer than
/// `timeout`, exceeds [`MAX_MIRROR_PACK_LEN`], or serves other bytes.
pub async fn fetch_from_mirror(
    base: &str,
    pack: InfoHash,
    pack_v2: Option<InfoHashV2>,
    out_dir: &Path,
    timeout: Duration,
) -> Result<PathBuf, Error> {
    let url = format!("{}/{pack}.pack", base.trim_end_matches('/'));
    let folder = out_dir.join(pack.to_string());
    fs::create_dir_all(&folder)?;
    // Named like a seeded pack, so the torrent it hashes to is the same.
    let file = folder.join("pack");
    let download = {
        let (url, file) = (url.clone(), file.clone());
        tokio::task::spawn_blocking(move || download(&url, &file, timeout))
    };
    download
        .await
        .map_err(|error| Error::Mirror(format!("GET {url}: {error}")))??;
    if let Err(error) = verify_pack(&file, pack, pack_v2).await {
        fs::remove_dir_all(&folder)?;
        return Err(Error::Mirror(format!(
            "{url} served a pack that fails: {error}"
        )));
    }
    Ok(file)
}

fn download(url: &str, file: &Path, timeout: Duration) -> Result<(), Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    let response = agent
        .get(url)
        .call()
        .map_err(|error| Error::Mirror(format!("GET {url}: {error}")))?;
    let mut body = response
        .into_body()
        .into_with_config()
        .limit(MAX_MIRROR_PACK_LEN)
        .reader();
    io::copy(&mut body, &mut File::create(file)?)
        .map_err(|error| Error::Mirror(format!("reading {url}: {error}")))?;
    Ok(())
}
