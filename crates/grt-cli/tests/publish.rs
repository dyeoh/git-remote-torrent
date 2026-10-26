//! `git torrent verify-mirror` and `git torrent publish`: the CI path that
//! publishes once, with packs served only by an HTTP mirror.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use grt_core::SignedManifest;
use grt_swarm::ManifestDht;
use mainline::Testnet;
use tempfile::TempDir;

/// Serves files under `root` over HTTP until the test exits.
fn serve(root: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(&stream);
            let (mut request, mut line) = (String::new(), String::new());
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let path = request.split(' ').nth(1).unwrap_or("/");
            let response = match fs::read(root.join(path.trim_start_matches('/'))) {
                Ok(body) => {
                    let mut response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    response.extend(body);
                    response
                }
                Err(_) => {
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec()
                }
            };
            let _ = stream.write_all(&response);
        }
    });
    base
}

fn torrent(dir: &Path, repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-torrent"))
        .current_dir(repo)
        .env("HOME", dir)
        .env("GIT_TORRENT_HOME", dir.join("torrent-home"))
        .env("GIT_CONFIG_GLOBAL", dir.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("run git-torrent")
}

fn git(dir: &Path, repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", dir.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_the_mirror_then_publish_once() {
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
        .await
        .expect("join");
    let temp = TempDir::new().expect("create temp dir");
    let dir = temp.path();
    fs::write(
        dir.join("gitconfig"),
        "[user]\n\tname = T\n\temail = t@example.com\n[init]\n\tdefaultBranch = main\n",
    )
    .expect("gitconfig");
    let repo = dir.join("repo");
    git(dir, dir, &["init", "-q", "repo"]);
    fs::write(repo.join("README"), "hi").expect("write");
    git(dir, &repo, &["add", "README"]);
    git(dir, &repo, &["commit", "-q", "-m", "first"]);
    let base = serve(dir.join("www"));
    let mirror = format!("{base}/packs");
    assert!(torrent(dir, &repo, &["key", "generate"]).status.success());
    assert!(
        torrent(dir, &repo, &["sign", "--mirror", &mirror])
            .status
            .success()
    );

    let output = tokio::task::block_in_place(|| torrent(dir, &repo, &["verify-mirror"]));
    assert!(
        !output.status.success() && text(&output).contains("cannot serve this manifest's packs"),
        "an empty mirror fails verification:\n{}",
        text(&output)
    );

    let www = dir.join("www").join("packs");
    let export = torrent(dir, &repo, &["export", www.to_str().expect("UTF-8")]);
    assert!(export.status.success());
    let output = tokio::task::block_in_place(|| torrent(dir, &repo, &["verify-mirror"]));
    assert!(
        output.status.success() && text(&output).contains("all 1 pack(s) of manifest #1 verified"),
        "{}",
        text(&output)
    );

    let mut args = vec!["publish"];
    for node in &testnet.bootstrap {
        args.extend(["--dht-bootstrap", node]);
    }
    let output = tokio::task::block_in_place(|| torrent(dir, &repo, &args));
    assert!(
        output.status.success() && text(&output).contains("Published manifest #1"),
        "{}",
        text(&output)
    );
    let signed =
        SignedManifest::decode(&fs::read(repo.join(".git/torrent/manifest")).expect("manifest"))
            .expect("verifies");
    let dht = ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST).expect("dht");
    let resolved = dht
        .resolve(&signed.public_key(), b"")
        .await
        .expect("resolve")
        .expect("published");
    assert_eq!(resolved, signed, "the signed manifest is on the DHT as is");

    // A pack corrupted on the mirror after upload is caught by the next check.
    let pack = fs::read_dir(&www)
        .expect("mirror dir")
        .next()
        .expect("one pack")
        .expect("entry")
        .path();
    let mut bytes = fs::read(&pack).expect("read");
    bytes[20] ^= 1;
    fs::write(&pack, bytes).expect("corrupt");
    let output = tokio::task::block_in_place(|| torrent(dir, &repo, &["verify-mirror"]));
    assert!(
        !output.status.success() && text(&output).contains("fails"),
        "{}",
        text(&output)
    );
}
