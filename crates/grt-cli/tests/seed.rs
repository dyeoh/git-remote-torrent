//! End-to-end: `git torrent seed` on a private DHT testnet, consumed by an
//! independent client that only knows the repository's address.

use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use grt_core::{ObjectId, PublicKey, RemoteUrl, SignedManifest};
use grt_git::Repo;
use grt_swarm::{ManifestDht, PackSwarm, SwarmConfig};
use mainline::Testnet;
use tempfile::TempDir;

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let dir = TempDir::new().expect("create temp dir");
        fs::write(
            dir.path().join("gitconfig"),
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n",
        )
        .expect("write gitconfig");
        let sandbox = Sandbox { dir };
        sandbox.git(&["init", "-q", "repo"], sandbox.dir.path());
        sandbox
    }

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

    fn git(&self, args: &[&str], cwd: &Path) -> String {
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
        String::from_utf8(output.stdout)
            .expect("UTF-8")
            .trim()
            .to_owned()
    }

    fn commit(&self, file: &str) -> ObjectId {
        let repo = self.path("repo");
        fs::write(repo.join(file), file).expect("write file");
        self.git(&["add", file], &repo);
        self.git(&["commit", "-q", "-m", file], &repo);
        self.git(&["rev-parse", "HEAD"], &repo)
            .parse()
            .expect("hex id")
    }

    /// Starts `git torrent seed` and returns it with a stream of its stdout lines.
    fn seed(&self, bootstrap: &[String]) -> (Child, Receiver<String>) {
        self.seed_in(&self.path("repo"), bootstrap, &["--listen", "127.0.0.1:0"])
    }

    /// Starts `git torrent seed` in `cwd` with `args`, on the testnet only.
    fn seed_in(
        &self,
        cwd: &Path,
        bootstrap: &[String],
        args: &[&str],
    ) -> (Child, Receiver<String>) {
        let mut command = self.command(env!("CARGO_BIN_EXE_git-torrent"), cwd);
        command.args(["seed", "--no-peer-dht"]).args(args);
        for node in bootstrap {
            command.args(["--dht-bootstrap", node]);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start git torrent seed");
        let stdout = child.stdout.take().expect("piped stdout");
        let (lines, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        (child, receiver)
    }
}

/// What a running seeder announced: its peer address and the repository address.
struct Announced {
    peer: SocketAddr,
    key: PublicKey,
}

/// Reads the seeder's output until it reports the manifest as published.
fn wait_for_publish(lines: &Receiver<String>) -> Announced {
    let (mut peer, mut key) = (None, None);
    loop {
        let line = lines
            .recv_timeout(Duration::from_secs(120))
            .expect("seeder should publish within two minutes");
        if let Some(address) = line.strip_prefix("Address: ")
            && let Ok(RemoteUrl::Swarm { key: found, .. }) = address.parse::<RemoteUrl>()
        {
            key = Some(found);
        }
        if let Some(rest) = line.strip_prefix("Seeding ") {
            let addr = rest
                .rsplit(" on ")
                .next()
                .expect("address")
                .trim_end_matches('.');
            peer = Some(addr.parse().expect("socket address"));
        }
        if line.starts_with("Published manifest") {
            return Announced {
                peer: peer.expect("seeding line came first"),
                key: key.expect("address line came first"),
            };
        }
    }
}

/// Resolves the manifest and rebuilds the repository from the swarm alone.
async fn clone_from_swarm(
    testnet: &Testnet,
    announced: &Announced,
    work: &Path,
) -> (SignedManifest, Repo) {
    let dht =
        ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST).expect("dht client");
    let signed = dht
        .resolve(&announced.key, b"")
        .await
        .expect("resolve")
        .expect("manifest is on the DHT");

    let mut config = SwarmConfig::default();
    config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
    config.dht = false;
    let swarm = PackSwarm::start(&work.join("client-state"), config)
        .await
        .expect("client swarm");
    let status = Command::new("git")
        .args(["init", "-q", "--bare", "clone.git"])
        .current_dir(work)
        .status()
        .expect("git init");
    assert!(status.success());
    let clone = Repo::discover(&work.join("clone.git")).expect("discover clone");
    for &pack in signed.manifest().packs() {
        let file = swarm
            .fetch(
                pack,
                &[announced.peer],
                &work.join("packs"),
                Duration::from_secs(60),
            )
            .await
            .expect("fetch pack");
        clone.index_pack(&file).expect("index pack in chain order");
    }
    swarm.stop().await;
    (signed, clone)
}

#[tokio::test(flavor = "multi_thread")]
async fn seeded_chain_rebuilds_the_repository() {
    let sandbox = Sandbox::new();
    let first = sandbox.commit("README");
    let status = sandbox
        .command(env!("CARGO_BIN_EXE_git-torrent"), &sandbox.path("repo"))
        .args(["key", "generate"])
        .output()
        .expect("generate key");
    assert!(status.status.success(), "key generate failed");
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
        .await
        .expect("join");

    // First publish: one full pack.
    let (mut seeder, lines) = sandbox.seed(&testnet.bootstrap);
    let announced = wait_for_publish(&lines);
    let work = TempDir::new().expect("client dir");
    let (signed, clone) = clone_from_swarm(&testnet, &announced, work.path()).await;
    seeder.kill().expect("stop seeder");
    seeder.wait().expect("reap seeder");
    assert_eq!(signed.seq().get(), 1);
    assert_eq!(
        signed.manifest().packs().len(),
        1,
        "first manifest has one pack"
    );
    assert!(
        clone.contains(&first).expect("query"),
        "commit rebuilt from the swarm"
    );

    // Second publish: the chain grows by an incremental pack.
    let second = sandbox.commit("CHANGES");
    let (mut seeder, lines) = sandbox.seed(&testnet.bootstrap);
    let announced = wait_for_publish(&lines);
    let work = TempDir::new().expect("client dir");
    let (signed, clone) = clone_from_swarm(&testnet, &announced, work.path()).await;
    seeder.kill().expect("stop seeder");
    seeder.wait().expect("reap seeder");
    assert_eq!(
        signed.seq().get(),
        2,
        "republished with the next sequence number"
    );
    assert_eq!(
        signed.manifest().packs().len(),
        2,
        "full pack plus one incremental pack"
    );
    let main = signed
        .manifest()
        .refs()
        .get(&"refs/heads/main".parse().expect("ref"));
    assert_eq!(
        main,
        Some(&second),
        "the manifest points main at the new commit"
    );
    assert!(
        clone.contains(&second).expect("query"),
        "new commit rebuilt from the chain"
    );
}

/// A free local port, for a seeder that must keep its address across restarts.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address")
        .port()
}

/// Reads `lines` until one starts with `prefix`, and returns it.
fn wait_for(lines: &Receiver<String>, prefix: &str) -> String {
    loop {
        let line = lines
            .recv_timeout(Duration::from_secs(120))
            .unwrap_or_else(|_| panic!("no line starting with {prefix:?} within two minutes"));
        if line.starts_with(prefix) {
            return line;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follower_keeps_a_cloned_repository_available() {
    let sandbox = Sandbox::new();
    let first = sandbox.commit("README");
    let status = sandbox
        .command(env!("CARGO_BIN_EXE_git-torrent"), &sandbox.path("repo"))
        .args(["key", "generate"])
        .output()
        .expect("generate key");
    assert!(status.status.success(), "key generate failed");
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
        .await
        .expect("join");
    // The publisher keeps one address, so the follower can ask it for packs.
    let publisher_addr = format!("127.0.0.1:{}", free_port());
    let publish = |sandbox: &Sandbox| {
        sandbox.seed_in(
            &sandbox.path("repo"),
            &testnet.bootstrap,
            &["--listen", &publisher_addr],
        )
    };
    let (mut publisher, lines) = publish(&sandbox);
    let announced = wait_for_publish(&lines);

    // A cloner's repository: a torrent:// remote, peers from its config. It
    // never fetched, so the follower downloads every pack itself.
    let cloner = sandbox.path("cloner");
    sandbox.git(&["init", "-q", "cloner"], sandbox.dir.path());
    let address = format!("torrent://{}", announced.key);
    sandbox.git(&["remote", "add", "origin", &address], &cloner);
    sandbox.git(&["config", "torrent.peer", &publisher_addr], &cloner);
    let (mut follower, follower_lines) = sandbox.seed_in(
        &cloner,
        &testnet.bootstrap,
        &[
            "--listen",
            "127.0.0.1:0",
            "--follow",
            "origin",
            "--republish-secs",
            "2",
        ],
    );
    let following = wait_for(&follower_lines, "Following origin: ");
    assert!(following.contains("manifest #1"), "{following}");
    let follower_peer: SocketAddr = following
        .rsplit(" on ")
        .next()
        .expect("address")
        .trim_end_matches('.')
        .parse()
        .expect("socket address");

    // With the publisher gone, the follower alone serves the repository.
    publisher.kill().expect("stop publisher");
    publisher.wait().expect("reap publisher");
    let from_follower = Announced {
        peer: follower_peer,
        key: announced.key,
    };
    let work = TempDir::new().expect("client dir");
    let (signed, clone) = clone_from_swarm(&testnet, &from_follower, work.path()).await;
    assert_eq!(signed.seq().get(), 1);
    assert!(
        clone.contains(&first).expect("query"),
        "cloned from the follower"
    );

    // The publisher signs #2 and seeds it briefly; the follower picks it up.
    let second = sandbox.commit("CHANGES");
    let (mut publisher, lines) = publish(&sandbox);
    wait_for_publish(&lines);
    let now = wait_for(&follower_lines, "Now seeding manifest #2");
    assert!(now.contains("2 pack(s)"), "{now}");
    publisher.kill().expect("stop publisher");
    publisher.wait().expect("reap publisher");

    let work = TempDir::new().expect("client dir");
    let (signed, clone) = clone_from_swarm(&testnet, &from_follower, work.path()).await;
    assert_eq!(signed.seq().get(), 2);
    assert!(
        clone.contains(&second).expect("query"),
        "the new commit came from the follower"
    );
    follower.kill().expect("stop follower");
    follower.wait().expect("reap follower");
}
