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
        let mut command = self.command(env!("CARGO_BIN_EXE_git-torrent"), &self.path("repo"));
        command.args(["seed", "--listen", "127.0.0.1:0", "--no-peer-dht"]);
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
