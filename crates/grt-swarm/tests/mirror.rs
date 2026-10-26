//! Packs from an HTTP mirror, served by a minimal in-test HTTP server.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use grt_core::InfoHash;
use grt_git::Repo;
use grt_swarm::{Error, fetch_from_mirror, info_hash_of};
use tempfile::TempDir;

/// Serves files under `root` at `http://127.0.0.1:<port>/<path>` until the
/// test exits, answering 404 for anything missing.
fn serve(root: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            // Drain the headers.
            let mut line = String::new();
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

/// A one-commit repository's full pack, stored as `<dir>/pack`.
async fn pack(dir: &Path) -> (PathBuf, InfoHash) {
    let source = dir.join("source");
    fs::create_dir(&source).expect("create source");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&source)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    fs::write(source.join("README"), "immortal\n").expect("write file");
    git(&["add", "README"]);
    git(&["commit", "-q", "-m", "one"]);
    let repo = Repo::discover(&source).expect("repo");
    let tip = repo.refs(&["refs/heads"]).expect("refs")[0].1;
    let file = dir.join("pack");
    repo.write_pack(&[tip], &[], &file).expect("write pack");
    let info_hash = info_hash_of(&file).await.expect("hash");
    (file, info_hash)
}

#[tokio::test(flavor = "multi_thread")]
async fn mirror_packs_are_verified_against_the_infohash() {
    let dir = TempDir::new().expect("create temp dir");
    let (file, info_hash) = pack(dir.path()).await;
    let served = dir.path().join("www").join("packs");
    fs::create_dir_all(&served).expect("create web root");
    let base = serve(dir.path().join("www"));
    let timeout = Duration::from_secs(10);

    fs::copy(&file, served.join(format!("{info_hash}.pack"))).expect("publish pack");
    let fetched = fetch_from_mirror(
        &format!("{base}/packs/"),
        info_hash,
        &dir.path().join("in"),
        timeout,
    )
    .await
    .expect("fetch from mirror");
    assert_eq!(
        fs::read(&fetched).expect("read"),
        fs::read(&file).expect("read"),
        "the mirror's pack arrives intact"
    );

    // A mirror serving other bytes under the right name is caught.
    let mut tampered = fs::read(&file).expect("read");
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    fs::write(served.join(format!("{info_hash}.pack")), tampered).expect("tamper");
    let out = dir.path().join("tampered");
    let result = fetch_from_mirror(&format!("{base}/packs"), info_hash, &out, timeout).await;
    assert!(
        matches!(result, Err(Error::Mirror(ref message)) if message.contains("served a pack")),
        "tampered pack must be rejected: {result:?}"
    );
    assert!(
        !out.join(info_hash.to_string()).exists(),
        "rejected bytes are deleted"
    );

    let missing = InfoHash::from([7; 20]);
    let result = fetch_from_mirror(&format!("{base}/packs"), missing, &out, timeout).await;
    assert!(
        matches!(result, Err(Error::Mirror(_))),
        "a 404 is a mirror error: {result:?}"
    );
}
