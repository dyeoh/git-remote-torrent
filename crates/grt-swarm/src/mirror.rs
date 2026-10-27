use std::fs::{self, File};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use grt_core::{InfoHash, InfoHashV2};
// The resolver API is outside ureq's semver guarantee; Cargo.lock pins it.
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

use crate::error::Error;
use crate::pack_swarm::verify_pack;

/// Largest pack accepted from a mirror, so a hostile one cannot fill the disk.
pub const MAX_MIRROR_PACK_LEN: u64 = 4 << 30;

/// Who chose a mirror, which decides the addresses it may connect to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirrorSource {
    /// Listed in a manifest, so chosen by its publisher. It may reach only
    /// public addresses, including after redirects, so a manifest cannot
    /// make cloners send requests to their loopback, LAN or cloud metadata
    /// services.
    Manifest,
    /// Configured by the user, who may point it anywhere.
    User,
}

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
/// `timeout`, exceeds [`MAX_MIRROR_PACK_LEN`], serves other bytes, or (for
/// [`MirrorSource::Manifest`]) would connect to a non-public address.
pub async fn fetch_from_mirror(
    base: &str,
    source: MirrorSource,
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
        tokio::task::spawn_blocking(move || download(&url, source, &file, timeout))
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

fn download(url: &str, source: MirrorSource, file: &Path, timeout: Duration) -> Result<(), Error> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build();
    let agent = match source {
        MirrorSource::Manifest => {
            ureq::Agent::with_parts(config, DefaultConnector::new(), PublicOnly::default())
        }
        MirrorSource::User => config.into(),
    };
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

/// Resolves host names like ureq does, keeping only public addresses. ureq
/// resolves again for every connection, so redirects are checked too.
#[derive(Debug, Default)]
struct PublicOnly(DefaultResolver);

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.0.resolve(uri, config, timeout)?;
        // A proxy the user configured may be anywhere; ureq resolves it here too.
        if config
            .proxy()
            .is_some_and(|proxy| uri.host() == Some(proxy.host()))
        {
            return Ok(addrs);
        }
        let mut public = self.empty();
        for addr in addrs.iter().filter(|addr| is_public(addr.ip())) {
            public.push(*addr);
        }
        if public.is_empty() {
            return Err(ureq::Error::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is not a public address, which a manifest's mirror must be \
                     (set torrent.mirror, or torrent.allowPrivateMirrors, to use it)",
                    uri.host().unwrap_or_default()
                ),
            )));
        }
        Ok(public)
    }
}

/// False for loopback, private, link-local, shared, multicast and other
/// special-purpose addresses, including IPv4 embedded in IPv6.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        // Carrier-grade NAT (100.64.0.0/10) and benchmarking (198.18.0.0/15).
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (b == 18 || b == 19))
        // IETF protocol assignments (192.0.0.0/24).
        || ip.octets()[..3] == [192, 0, 0])
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // IPv4-mapped (::ffff:0:0/96), NAT64 (64:ff9b::/96) and 6to4 (2002::/16)
    // addresses reach an IPv4 address, so that one decides.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let octets = ip.octets();
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }
    if segments[0] == 0x2002 {
        return is_public_v4(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]));
    }
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        // IPv4-compatible (::/96), deprecated.
        || segments[..6] == [0; 6]
        // Unique local (fc00::/7), link-local (fe80::/10), site-local (fec0::/10).
        || segments[0] & 0xfe00 == 0xfc00
        || segments[0] & 0xffc0 == 0xfe80
        || segments[0] & 0xffc0 == 0xfec0
        // Documentation (2001:db8::/32).
        || segments[..2] == [0x2001, 0xdb8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_non_public_addresses() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.251",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "64:ff9b::a00:1",
            "2002:c0a8:101::1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip} must be refused");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for ip in [
            "1.1.1.1",
            "140.82.112.3",
            "2606:4700:4700::1111",
            "::ffff:1.1.1.1",
            "64:ff9b::101:101",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip} must be allowed");
        }
    }

    #[tokio::test]
    async fn a_manifest_mirror_cannot_reach_loopback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let dir = tempfile::TempDir::new().unwrap();
        let pack = InfoHash::from([1; 20]);
        let error = fetch_from_mirror(
            &base,
            MirrorSource::Manifest,
            pack,
            None,
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("is not a public address"),
            "{error}"
        );
        let error = fetch_from_mirror(
            "http://localhost:9",
            MirrorSource::Manifest,
            pack,
            None,
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("is not a public address"),
            "names resolving to loopback are refused too: {error}"
        );
    }
}
