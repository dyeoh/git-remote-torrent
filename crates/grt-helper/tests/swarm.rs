//! End-to-end: real `git` cloning and fetching through `git-remote-torrent`
//! from a publisher on a private DHT testnet, with and without an upstream.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use grt_core::{
    InfoHashV2, Manifest, ObjectId, PublisherKey, Seq, SignedManifest, TRUSTED_KEYS_FILE,
    TrustStore,
};
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig};
use mainline::Testnet;
use tempfile::TempDir;

/// An isolated git environment with a source repository and the helper on PATH.
struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let dir = TempDir::new().expect("create temp dir");
        fs::write(
            dir.path().join("gitconfig"),
            "[user]\n\tname = Test\n\temail = test@example.com\n\
             [init]\n\tdefaultBranch = main\n\
             [protocol \"file\"]\n\tallow = always\n",
        )
        .expect("write sandbox gitconfig");
        let sandbox = Sandbox { dir };
        sandbox.ok(sandbox.dir.path(), &["init", "-q", "source"]);
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Output {
        let helper = PathBuf::from(env!("CARGO_BIN_EXE_git-remote-torrent"));
        let mut path = OsString::from(helper.parent().expect("helper binary has a parent dir"));
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("run git")
    }

    /// Runs git, panics with its stderr if it fails, and returns its stderr.
    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.git(cwd, args);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "git {args:?} failed:\n{stderr}");
        stderr
    }

    fn rev_parse(&self, repo: &str, rev: &str) -> ObjectId {
        let output = self.git(&self.path(repo), &["rev-parse", rev]);
        assert!(output.status.success(), "rev-parse {rev} in {repo}");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("hex id")
    }

    fn commit(&self, file: &str) -> ObjectId {
        let source = self.path("source");
        fs::write(source.join(file), file).expect("write work tree file");
        self.ok(&source, &["add", file]);
        self.ok(&source, &["commit", "-q", "-m", file]);
        self.rev_parse("source", "HEAD")
    }

    /// Runs git with the swarm settings that reach `publisher`, with a short
    /// deadline so a missing seeder fails fast.
    fn swarm_git(&self, publisher: &Publisher, cwd: &Path, args: &[&str]) -> Output {
        let mut full: Vec<String> = Vec::new();
        for node in &publisher.bootstrap {
            full.extend(["-c".to_owned(), format!("torrent.dhtBootstrap={node}")]);
        }
        full.extend([
            "-c".to_owned(),
            format!("torrent.peer={}", publisher.peer),
            "-c".to_owned(),
            "torrent.peerDht=false".to_owned(),
            "-c".to_owned(),
            "torrent.fetchTimeout=10".to_owned(),
        ]);
        full.extend(args.iter().map(|&arg| arg.to_owned()));
        let full: Vec<&str> = full.iter().map(String::as_str).collect();
        // Git blocks; keep the publisher's tasks running on other workers.
        tokio::task::block_in_place(|| self.git(cwd, &full))
    }

    fn swarm_ok(&self, publisher: &Publisher, cwd: &Path, args: &[&str]) -> String {
        let output = self.swarm_git(publisher, cwd, args);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "git {args:?} failed:\n{stderr}");
        stderr
    }
}

/// Publishes the source repository the way `git torrent seed` does: a pack
/// chain seeded on loopback and a signed manifest on the testnet.
struct Publisher {
    key: PublisherKey,
    name: String,
    upstream: Option<String>,
    mirrors: Vec<String>,
    /// Sign a wrong SHA-256 (v2) infohash for every pack.
    wrong_sha256: bool,
    bootstrap: Vec<String>,
    swarm: PackSwarm,
    peer: SocketAddr,
    dht: ManifestDht,
    signed: Option<SignedManifest>,
    packs: PathBuf,
    _testnet: Testnet,
}

impl Publisher {
    async fn start(sandbox: &Sandbox, name: &str, upstream: Option<String>) -> Publisher {
        let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
            .await
            .expect("join");
        let mut config = SwarmConfig::default();
        config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
        config.dht = false;
        let swarm = PackSwarm::start(&sandbox.path("publisher-state"), config)
            .await
            .expect("publisher swarm");
        let peer = swarm.listen_addr().expect("listening");
        let dht = ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST)
            .expect("publisher dht");
        Publisher {
            key: PublisherKey::generate().expect("key"),
            name: name.to_owned(),
            upstream,
            mirrors: Vec::new(),
            wrong_sha256: false,
            bootstrap: testnet.bootstrap.clone(),
            swarm,
            peer,
            dht,
            signed: None,
            packs: sandbox.path("publisher-packs"),
            _testnet: testnet,
        }
    }

    /// The address clients use.
    fn address(&self) -> String {
        let key = self.key.public_key();
        if self.name.is_empty() {
            format!("torrent://{key}")
        } else {
            format!("torrent://{key}/{}", self.name)
        }
    }

    /// Packs what the source gained since the last publish, seeds it, and
    /// publishes the next manifest.
    async fn publish(&mut self, sandbox: &Sandbox) {
        let repo = Repo::discover(&sandbox.path("source")).expect("source repo");
        let mut manifest = Manifest::new();
        for (name, id) in repo.refs(&["refs/heads", "refs/tags"]).expect("refs") {
            manifest.insert_ref(name, id).expect("ref fits");
        }
        manifest
            .set_head(repo.head_target().expect("head"))
            .expect("head is a published ref");
        manifest.set_upstream(self.upstream.clone());
        manifest
            .set_mirrors(self.mirrors.clone())
            .expect("valid mirrors");

        let (mut packs, exclude, seq) = match &self.signed {
            Some(previous) => (
                previous.manifest().packs().to_vec(),
                previous.manifest().refs().values().copied().collect(),
                previous.seq().next().expect("seq"),
            ),
            None => (Vec::new(), Vec::new(), Seq::new(1)),
        };
        let tips: Vec<ObjectId> = manifest.refs().values().copied().collect();
        let dir = self.packs.join(packs.len().to_string());
        fs::create_dir_all(&dir).expect("pack dir");
        let file = dir.join("pack");
        repo.write_pack(&tips, &exclude, &file).expect("write pack");
        packs.push(self.swarm.seed(&file).await.expect("seed"));
        for &pack in &packs {
            manifest.push_pack(pack);
        }
        // Record the SHA-256 (v2) infohashes, as `git torrent sign` does.
        let mut sha256 = Vec::new();
        for index in 0..packs.len() {
            let file = self.packs.join(index.to_string()).join("pack");
            let real = grt_swarm::hashes_of(&file).await.expect("hash").v2;
            sha256.push(if self.wrong_sha256 {
                InfoHashV2::from([7; 32])
            } else {
                real
            });
        }
        manifest.set_packs_v2(Some(sha256)).expect("one per pack");

        let signed = self
            .key
            .sign(&manifest, seq, self.name.as_bytes())
            .expect("sign");
        let cas = self.signed.as_ref().map(SignedManifest::seq);
        self.dht.publish(&signed, cas).await.expect("publish");
        self.signed = Some(signed);
    }
}

impl Publisher {
    /// Lays the chain out as `<dir>/<infohash>.pack`, like `git torrent export`.
    fn export(&self, dir: &Path) {
        fs::create_dir_all(dir).expect("export dir");
        let signed = self.signed.as_ref().expect("published");
        for (index, pack) in signed.manifest().packs().iter().enumerate() {
            fs::copy(
                self.packs.join(index.to_string()).join("pack"),
                dir.join(format!("{pack}.pack")),
            )
            .expect("export pack");
        }
    }
}

/// Serves files under `root` over HTTP until the test exits.
fn serve(root: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            let mut line = String::new();
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

fn clone_head(sandbox: &Sandbox) -> ObjectId {
    sandbox.rev_parse("clone", "HEAD")
}

#[tokio::test(flavor = "multi_thread")]
async fn pure_clone_and_incremental_fetch_from_the_swarm() {
    let sandbox = Sandbox::new();
    let first = sandbox.commit("README");
    let mut publisher = Publisher::start(&sandbox, "demo", None).await;
    publisher.publish(&sandbox).await;

    let address = publisher.address();
    let stderr = sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &["clone", &address, "clone"],
    );
    assert!(
        stderr.contains("Fetched 1 pack(s) of manifest #1 from the swarm"),
        "cloned from the swarm:\n{stderr}"
    );
    assert!(
        stderr.contains("hint: keep this repository available to others: git torrent seed --follow origin --install"),
        "a clone from the swarm suggests following:\n{stderr}"
    );
    assert_eq!(
        clone_head(&sandbox),
        first,
        "HEAD checked out from the swarm"
    );
    assert_eq!(
        fs::read_to_string(sandbox.path("clone").join("README")).expect("work tree"),
        "README"
    );

    let second = sandbox.commit("CHANGES");
    publisher.publish(&sandbox).await;
    sandbox.ok(
        &sandbox.path("clone"),
        &["config", "torrent.keepPacks", "true"],
    );
    let stderr = sandbox.swarm_ok(&publisher, &sandbox.path("clone"), &["pull", "-q"]);
    assert!(
        stderr.contains("Fetched 1 pack(s) of manifest #2 from the swarm"),
        "only the new pack of the chain is downloaded:\n{stderr}"
    );
    assert!(
        !stderr.contains("hint:"),
        "keeping packs already means following:\n{stderr}"
    );
    let kept: Vec<_> = fs::read_dir(sandbox.path("clone").join(".git/torrent/packs"))
        .expect("pack store")
        .map(|entry| entry.expect("entry").path().join("pack"))
        .collect();
    assert_eq!(kept.len(), 1, "the new pack is kept: {kept:?}");
    assert!(kept[0].is_file(), "stored as <infohash>/pack");
    assert_eq!(
        clone_head(&sandbox),
        second,
        "fast-forwarded from the swarm"
    );

    let clone = sandbox.path("clone");
    fs::write(clone.join("LOCAL"), "local").expect("write work tree file");
    sandbox.ok(&clone, &["add", "LOCAL"]);
    sandbox.ok(&clone, &["commit", "-q", "-m", "local"]);
    let output = sandbox.swarm_git(&publisher, &clone, &["push", "origin", "main"]);
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("not yours to publish"),
        "pushing to someone else's swarm remote is refused with a reason"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn hybrid_clone_survives_an_upstream_takedown() {
    let sandbox = Sandbox::new();
    let first = sandbox.commit("README");
    let upstream = sandbox.path("upstream.git");
    sandbox.ok(
        sandbox.dir.path(),
        &["clone", "-q", "--bare", "source", "upstream.git"],
    );
    let url = format!("file://{}", upstream.display());
    let mut publisher = Publisher::start(&sandbox, "", Some(url.clone())).await;
    publisher.publish(&sandbox).await;

    // The kill switch: the upstream disappears.
    fs::rename(&upstream, sandbox.path("taken-down.git")).expect("take the upstream down");

    let address = format!("remote.origin.torrentAddress={}", publisher.address());
    let stderr = sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &["clone", "-c", &address, &format!("torrent::{url}"), "clone"],
    );
    assert!(
        stderr.contains("the upstream is unavailable")
            && stderr.contains("serving manifest #1 from the swarm"),
        "the user is told where the code came from:\n{stderr}"
    );
    assert_eq!(clone_head(&sandbox), first, "cloned without the upstream");
}

#[tokio::test(flavor = "multi_thread")]
async fn hybrid_fetch_prefers_the_swarm_and_falls_back_to_the_upstream() {
    let sandbox = Sandbox::new();
    let first = sandbox.commit("README");
    sandbox.ok(
        sandbox.dir.path(),
        &["clone", "-q", "--bare", "source", "upstream.git"],
    );
    let url = format!("file://{}", sandbox.path("upstream.git").display());
    let mut publisher = Publisher::start(&sandbox, "", Some(url.clone())).await;
    publisher.publish(&sandbox).await;
    let address = format!("remote.origin.torrentAddress={}", publisher.address());
    let remote = format!("torrent::{url}");

    let stderr = sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &["clone", "-c", &address, &remote, "clone"],
    );
    assert!(
        stderr.contains("from the swarm") && !stderr.contains("unavailable"),
        "objects came from the swarm while the upstream listed refs:\n{stderr}"
    );
    assert_eq!(clone_head(&sandbox), first);

    // The seeder goes away; the upstream still has everything.
    publisher.swarm.stop().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stderr = sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &["clone", "-c", &address, &remote, "fallback"],
    );
    assert!(
        stderr.contains("fetching from the swarm failed"),
        "the swarm failure is reported before falling back:\n{stderr}"
    );
    assert_eq!(
        sandbox.rev_parse("fallback", "HEAD"),
        first,
        "the upstream served the clone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn http_mirror_serves_the_clone_when_upstream_and_peers_are_gone() {
    let sandbox = Sandbox::new();
    let first = sandbox.commit("README");
    let upstream = sandbox.path("upstream.git");
    sandbox.ok(
        sandbox.dir.path(),
        &["clone", "-q", "--bare", "source", "upstream.git"],
    );
    let url = format!("file://{}", upstream.display());
    let base = serve(sandbox.path("www"));
    let mut publisher = Publisher::start(&sandbox, "", Some(url.clone())).await;
    publisher.mirrors = vec![format!("{base}/packs")];
    publisher.publish(&sandbox).await;
    publisher.export(&sandbox.path("www").join("packs"));

    // Everything but the DHT and a static file host goes away.
    publisher.swarm.stop().await;
    fs::rename(&upstream, sandbox.path("taken-down.git")).expect("take the upstream down");

    let address = format!("remote.origin.torrentAddress={}", publisher.address());
    let upstream = format!("torrent::{url}");

    // The manifest's mirror is on loopback, which a publisher may not make
    // cloners connect to.
    let output = sandbox.swarm_git(
        &publisher,
        sandbox.dir.path(),
        &["clone", "-c", &address, &upstream, "clone"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("is not a public address"),
        "a manifest mirror on loopback is refused:\n{stderr}"
    );

    // The user can allow it, or name the mirror themselves.
    let stderr = sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &[
            "clone",
            "-c",
            &address,
            "-c",
            "torrent.allowPrivateMirrors=true",
            "-c",
            "torrent.seedHint=false",
            &upstream,
            "clone",
        ],
    );
    assert!(
        stderr.contains("the upstream is unavailable")
            && stderr.contains("Fetched 1 pack(s) of manifest #1 from HTTP mirrors."),
        "the pack came from the mirror the manifest lists:\n{stderr}"
    );
    assert!(
        !stderr.contains("hint:"),
        "torrent.seedHint=false:\n{stderr}"
    );
    assert_eq!(clone_head(&sandbox), first, "cloned from the mirror");
    let mirror = format!("torrent.mirror={base}/packs");
    sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &[
            "clone",
            "-c",
            &address,
            "-c",
            &mirror,
            &upstream,
            "by-config",
        ],
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pack_failing_its_sha256_is_refused() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    let mut publisher = Publisher::start(&sandbox, "", None).await;
    publisher.wrong_sha256 = true;
    publisher.publish(&sandbox).await;

    let address = publisher.address();
    let output = sandbox.swarm_git(
        &publisher,
        sandbox.dir.path(),
        &["clone", &address, "clone"],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("SHA-256 (v2) infohash"),
        "the right SHA-1 is not enough; the signed SHA-256 must match too:\n{stderr}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_key_is_pinned_and_the_trust_store_decides() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    let upstream = sandbox.path("upstream.git");
    sandbox.ok(
        sandbox.dir.path(),
        &["clone", "-q", "--bare", "source", "upstream.git"],
    );
    let url = format!("file://{}", upstream.display());
    let mut publisher = Publisher::start(&sandbox, "", Some(url.clone())).await;
    publisher.publish(&sandbox).await;
    let key = publisher.key.public_key();

    // A clone while the upstream answers pins the address's key for it.
    let address = format!("remote.origin.torrentAddress={}", publisher.address());
    sandbox.swarm_ok(
        &publisher,
        sandbox.dir.path(),
        &["clone", "-c", &address, &format!("torrent::{url}"), "clone"],
    );
    let pins = fs::read_to_string(sandbox.path("clone/.git/torrent/pins")).expect("pins");
    assert!(pins.contains(&format!("{key} {url}")), "{pins}");

    // Someone re-signs the same manifest with their own key, and talks the
    // user into using it; then the upstream goes away.
    let impostor = PublisherKey::generate().expect("key");
    let forged = impostor
        .sign(
            publisher.signed.as_ref().expect("published").manifest(),
            Seq::new(1),
            b"",
        )
        .expect("sign");
    publisher.dht.publish(&forged, None).await.expect("publish");
    fs::rename(&upstream, sandbox.path("taken-down.git")).expect("take the upstream down");

    let clone = sandbox.path("clone");
    let impostor_address = format!(
        "remote.origin.torrentAddress=torrent://{}",
        impostor.public_key()
    );
    let output = sandbox.swarm_git(&publisher, &clone, &["-c", &impostor_address, "fetch"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success()
            && stderr.contains("note: refs are signed by ssh-ed25519")
            && stderr.contains("was first fetched with key"),
        "a key other than the pinned one is refused, and shown:\n{stderr}"
    );
    sandbox.swarm_ok(&publisher, &clone, &["fetch"]);

    // A trust store with entries decides instead of the pin.
    let mut store = TrustStore::new();
    store.insert(impostor.public_key(), "new key");
    fs::write(
        clone.join(".git/torrent").join(TRUSTED_KEYS_FILE),
        store.to_string(),
    )
    .expect("trust store");
    let output = sandbox.swarm_git(&publisher, &clone, &["fetch"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("not in this repository's trusted keys"),
        "the pinned key is refused once it is not in the trust store:\n{stderr}"
    );
    sandbox.swarm_ok(&publisher, &clone, &["-c", &impostor_address, "fetch"]);
}
