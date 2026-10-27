//! `git torrent adopt`: a second machine with the key (a CI runner) takes
//! over an address from the DHT and an HTTP mirror, then extends its chain.

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

struct Sandbox {
    dir: TempDir,
    bootstrap: Vec<String>,
}

impl Sandbox {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("HOME", self.dir.path())
            .env("GIT_TORRENT_HOME", self.path("torrent-home"))
            .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }

    fn git(&self, cwd: &Path, args: &[&str]) {
        let output = self
            .command("git", cwd)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn commit(&self, repo: &str, file: &str) {
        let repo = self.path(repo);
        fs::write(repo.join(file), file).expect("write file");
        self.git(&repo, &["add", file]);
        self.git(&repo, &["commit", "-q", "-m", file]);
    }

    fn torrent(&self, repo: &str, args: &[&str]) -> Output {
        let mut command = self.command(env!("CARGO_BIN_EXE_git-torrent"), &self.path(repo));
        command.args(args);
        if args.first() == Some(&"adopt") {
            for node in &self.bootstrap {
                command.args(["--dht-bootstrap", node]);
            }
        }
        command.output().expect("run git-torrent")
    }

    fn ok(&self, repo: &str, args: &[&str]) -> String {
        let output = self.torrent(repo, args);
        assert!(
            output.status.success(),
            "git torrent {args:?} in {repo}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_machine_adopts_the_chain_and_extends_it() {
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
        .await
        .expect("join");
    let sandbox = Sandbox {
        dir: TempDir::new().expect("create temp dir"),
        bootstrap: testnet.bootstrap.clone(),
    };
    fs::write(
        sandbox.path("gitconfig"),
        "[user]\n\tname = T\n\temail = t@example.com\n[init]\n\tdefaultBranch = main\n\
         [protocol \"file\"]\n\tallow = always\n",
    )
    .expect("gitconfig");
    let base = serve(sandbox.path("www"));
    let mirror = format!("{base}/packs");

    // The laptop: two signs, packs exported to the mirror, manifest published.
    sandbox.git(sandbox.dir.path(), &["init", "-q", "laptop"]);
    sandbox.commit("laptop", "README");
    sandbox.ok("laptop", &["key", "generate"]);
    sandbox.ok("laptop", &["sign", "--mirror", &mirror]);
    sandbox.commit("laptop", "CHANGES");
    sandbox.ok("laptop", &["sign"]);
    let www = sandbox.path("www").join("packs");
    sandbox.ok("laptop", &["export", www.to_str().expect("UTF-8")]);
    let signed = SignedManifest::decode(
        &fs::read(sandbox.path("laptop/.git/torrent/manifest")).expect("manifest"),
    )
    .expect("verifies");
    let dht = ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST).expect("dht");
    dht.publish(&signed, None).await.expect("publish");

    // The runner: a full clone with the same key adopts, then signs #3.
    sandbox.git(sandbox.dir.path(), &["clone", "-q", "laptop", "runner"]);
    let out = tokio::task::block_in_place(|| sandbox.ok("runner", &["adopt"]));
    assert!(
        out.contains("Adopted manifest #2") && out.contains("2 pack(s)"),
        "{out}"
    );
    let out = tokio::task::block_in_place(|| sandbox.ok("runner", &["adopt"]));
    assert!(
        out.contains("Already up to date"),
        "adopting twice is a no-op: {out}"
    );

    sandbox.commit("runner", "FROM-CI");
    let out = sandbox.ok("runner", &["sign"]);
    assert!(
        out.contains("Added pack")
            && out.contains("Signed manifest #3")
            && out.contains("3 pack(s)"),
        "the runner continues the laptop's chain: {out}"
    );

    // A shallow checkout must not sign: its packs would lack history.
    let laptop_url = format!("file://{}", sandbox.path("laptop").display());
    sandbox.git(
        sandbox.dir.path(),
        &["clone", "-q", "--depth", "1", &laptop_url, "shallow"],
    );
    let output = sandbox.torrent("shallow", &["sign"]);
    assert!(
        !output.status.success() && String::from_utf8_lossy(&output.stderr).contains("shallow"),
        "shallow clones are refused"
    );

    // A corrupted mirror is caught, and nothing is adopted.
    let first = www.join(format!("{}.pack", signed.manifest().packs()[0]));
    let mut bytes = fs::read(&first).expect("read pack");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&first, bytes).expect("corrupt pack");
    sandbox.git(sandbox.dir.path(), &["clone", "-q", "laptop", "other"]);
    let output = tokio::task::block_in_place(|| sandbox.torrent("other", &["adopt"]));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("no source has pack"),
        "a corrupt mirror fails the adoption:\n{stderr}"
    );
    assert!(
        !sandbox.path("other/.git/torrent/manifest").exists(),
        "nothing is half-adopted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adopts_from_a_copy_when_the_dht_has_nothing() {
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
        .await
        .expect("join");
    let sandbox = Sandbox {
        dir: TempDir::new().expect("create temp dir"),
        bootstrap: testnet.bootstrap.clone(),
    };
    fs::write(
        sandbox.path("gitconfig"),
        "[user]\n\tname = T\n\temail = t@example.com\n[init]\n\tdefaultBranch = main\n\
         [protocol \"file\"]\n\tallow = always\n",
    )
    .expect("gitconfig");
    let base = serve(sandbox.path("www"));
    let mirror = format!("{base}/packs");

    // CI's first run: nothing published, so there is nothing to adopt.
    sandbox.git(sandbox.dir.path(), &["init", "-q", "laptop"]);
    sandbox.commit("laptop", "README");
    sandbox.ok("laptop", &["key", "generate"]);
    let output = tokio::task::block_in_place(|| sandbox.torrent("laptop", &["adopt"]));
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("nothing is published"),
        "adopting nothing fails by default"
    );
    let out = tokio::task::block_in_place(|| sandbox.ok("laptop", &["adopt", "--allow-new"]));
    assert!(out.contains("Nothing is published"), "{out}");

    // Signed and mirrored with a copy of the manifest, but never published
    // (or long expired).
    sandbox.ok("laptop", &["sign", "--mirror", &mirror]);
    let www = sandbox.path("www").join("packs");
    sandbox.ok("laptop", &["export", www.to_str().expect("UTF-8")]);
    let copy = www.join("manifest");
    fs::copy(sandbox.path("laptop/.git/torrent/manifest"), &copy).expect("copy manifest");
    let copy = copy.to_str().expect("UTF-8");

    sandbox.git(sandbox.dir.path(), &["clone", "-q", "laptop", "runner"]);
    let output = tokio::task::block_in_place(|| {
        sandbox.torrent("runner", &["adopt", "--name", "other", "--manifest", copy])
    });
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("is a manifest of"),
        "a copy of another address is refused"
    );
    let out = tokio::task::block_in_place(|| {
        sandbox.ok("runner", &["adopt", "--allow-new", "--manifest", copy])
    });
    assert!(
        out.contains("Adopted manifest #1") && out.contains("1 pack(s)"),
        "{out}"
    );
    sandbox.commit("runner", "FROM-CI");
    let out = sandbox.ok("runner", &["sign"]);
    assert!(
        out.contains("Signed manifest #2") && out.contains("2 pack(s)"),
        "the copy's chain continues: {out}"
    );
}
