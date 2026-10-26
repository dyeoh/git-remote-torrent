//! End-to-end: `git push` to swarm remotes, with a real `git torrent seed`
//! publishing what the pushes sign, on a private DHT testnet.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use mainline::Testnet;
use tempfile::TempDir;

/// The directory holding both `git-remote-torrent` and `git-torrent`.
fn bin_dir() -> PathBuf {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_git-remote-torrent"));
    let dir = helper.parent().expect("binary dir").to_path_buf();
    if !dir.join("git-torrent").exists() {
        // Cargo builds only this package's binaries for its tests.
        let status = Command::new(env!("CARGO"))
            .args(["build", "-q", "-p", "grt-cli", "--bin", "git-torrent"])
            .status()
            .expect("run cargo");
        assert!(status.success(), "building git-torrent failed");
    }
    dir
}

struct Sandbox {
    dir: TempDir,
    bootstrap: Vec<String>,
    _testnet: Testnet,
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
        let testnet = Testnet::builder(5).build().expect("testnet");
        let sandbox = Sandbox {
            dir,
            bootstrap: testnet.bootstrap.clone(),
            _testnet: testnet,
        };
        sandbox.ok(sandbox.dir.path(), &["init", "-q", "source"]);
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut path = OsString::from(bin_dir());
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("GIT_TORRENT_HOME", self.path("torrent-home"))
            .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    /// Runs git with the testnet as the DHT and `peer` as the only pack source.
    fn git_via(&self, peer: Option<&str>, cwd: &Path, args: &[&str]) -> Output {
        let mut command = self.command("git", cwd);
        for node in &self.bootstrap {
            command.args(["-c", &format!("torrent.dhtBootstrap={node}")]);
        }
        if let Some(peer) = peer {
            command.args(["-c", &format!("torrent.peer={peer}")]);
        }
        command
            .args([
                "-c",
                "torrent.peerDht=false",
                "-c",
                "torrent.fetchTimeout=20",
            ])
            .args(args)
            .output()
            .expect("run git")
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> Output {
        self.git_via(None, cwd, args)
    }

    /// Runs git, panics with its stderr on failure, and returns stderr.
    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        check(&self.git(cwd, args), args)
    }

    fn rev_parse(&self, repo: &str, rev: &str) -> String {
        let output = self.git(&self.path(repo), &["rev-parse", rev]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn commit(&self, file: &str) -> String {
        let source = self.path("source");
        fs::write(source.join(file), file).expect("write work tree file");
        self.ok(&source, &["add", file]);
        self.ok(&source, &["commit", "-q", "-m", file]);
        self.rev_parse("source", "HEAD")
    }

    fn torrent(&self, args: &[&str]) -> String {
        let output = self
            .command("git-torrent", &self.path("source"))
            .args(args)
            .output()
            .expect("run git-torrent");
        check(&output, args);
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Starts `git torrent seed` on loopback and returns it with its stdout lines.
    fn seed(&self) -> (Child, Receiver<String>) {
        self.seed_with(&self.path("source"), &[])
    }

    /// Starts `git torrent seed` on loopback in `cwd`, with extra `args`, on the
    /// testnet; returns it with a stream of its stdout lines.
    fn seed_with(&self, cwd: &Path, args: &[&str]) -> (Child, Receiver<String>) {
        let mut command = self.command("git-torrent", cwd);
        command.args(["seed", "--listen", "127.0.0.1:0", "--no-peer-dht"]);
        command.args(args);
        for node in &self.bootstrap {
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

fn check(output: &Output, args: &[&str]) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{args:?} failed:\n{stderr}");
    stderr
}

/// Waits for a seeder line starting with `prefix` and returns it.
fn wait_for(lines: &Receiver<String>, prefix: &str) -> String {
    loop {
        let line = lines
            .recv_timeout(Duration::from_secs(120))
            .unwrap_or_else(|_| panic!("seeder never printed {prefix:?}"));
        if line.starts_with(prefix) {
            return line;
        }
    }
}

#[test]
fn push_to_a_pure_swarm_remote_is_seeded_and_published() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    let address = sandbox
        .torrent(&["key", "generate"])
        .lines()
        .find_map(|line| {
            line.strip_prefix("Address:")
                .map(|rest| rest.trim().to_owned())
        })
        .expect("key generate prints the address");

    let (mut seeder, lines) = sandbox.seed();
    let peer = wait_for(&lines, "Seeding ")
        .rsplit(" on ")
        .next()
        .expect("address")
        .trim_end_matches('.')
        .to_owned();
    wait_for(&lines, "Published manifest #1");

    let source = sandbox.path("source");
    sandbox.ok(&source, &["remote", "add", "swarm", &address]);
    sandbox.ok(&source, &["config", "remote.swarm.torrentPublish", "true"]);

    // A fast-forward push signs manifest #2; the running seeder publishes it.
    let second = sandbox.commit("CHANGES");
    let stderr = check(
        &sandbox.git_via(Some(&peer), &source, &["push", "swarm", "main"]),
        &["push"],
    );
    assert!(
        stderr.contains("Signed manifest #2")
            && stderr.contains("running `git torrent seed` will publish"),
        "the push signs and hands over to the seeder:\n{stderr}"
    );
    wait_for(&lines, "Published manifest #2");

    let stderr = check(
        &sandbox.git_via(
            Some(&peer),
            sandbox.dir.path(),
            &["clone", &address, "clone"],
        ),
        &["clone"],
    );
    assert!(stderr.contains("manifest #2"), "{stderr}");
    assert_eq!(
        sandbox.rev_parse("clone", "HEAD"),
        second,
        "a fresh clone sees the pushed commit"
    );

    // Rewriting published history needs a force push, as on any git server.
    sandbox.ok(&source, &["reset", "-q", "--hard", "HEAD~1"]);
    let rewritten = sandbox.commit("REWRITTEN");
    let output = sandbox.git_via(Some(&peer), &source, &["push", "swarm", "main"]);
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("non-fast-forward"),
        "a rewind is rejected:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    check(
        &sandbox.git_via(Some(&peer), &source, &["push", "swarm", "+main"]),
        &["push", "+main"],
    );
    wait_for(&lines, "Published manifest #3");

    // Someone without publish rights is refused before anything is signed.
    let clone = sandbox.path("clone");
    let output = sandbox.git_via(
        Some(&peer),
        &clone,
        &["push", "origin", "HEAD:refs/heads/x"],
    );
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains("not yours to publish"),
        "clones cannot publish:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    seeder.kill().expect("stop seeder");
    seeder.wait().expect("reap seeder");
    assert_ne!(rewritten, second);
}

#[test]
fn hybrid_push_updates_the_upstream_and_signs_the_manifest() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    let address = sandbox
        .torrent(&["key", "generate"])
        .lines()
        .find_map(|line| {
            line.strip_prefix("Address:")
                .map(|rest| rest.trim().to_owned())
        })
        .expect("key generate prints the address");
    sandbox.ok(
        sandbox.dir.path(),
        &["init", "-q", "--bare", "upstream.git"],
    );
    let source = sandbox.path("source");
    let url = format!("torrent::file://{}", sandbox.path("upstream.git").display());
    sandbox.ok(&source, &["remote", "add", "origin", &url]);
    sandbox.ok(
        &source,
        &["config", "remote.origin.torrentAddress", &address],
    );
    sandbox.ok(&source, &["config", "remote.origin.torrentPublish", "true"]);

    let stderr = sandbox.ok(&source, &["push", "origin", "main"]);
    assert!(
        stderr.contains("Signed manifest #1") && stderr.contains("Run `git torrent seed`"),
        "with no seeder running, the user is told how to publish:\n{stderr}"
    );
    let upstream_main = sandbox.git(
        &sandbox.path("upstream.git"),
        &["rev-parse", "refs/heads/main"],
    );
    assert_eq!(
        String::from_utf8_lossy(&upstream_main.stdout).trim(),
        sandbox.rev_parse("source", "HEAD"),
        "the upstream got the push"
    );
    let status = sandbox.torrent(&["status"]);
    assert!(
        status.contains("refs/heads/main"),
        "the signed manifest holds the pushed ref:\n{status}"
    );
}

#[test]
fn push_while_the_upstream_is_down_and_reconcile_later() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    let address = sandbox
        .torrent(&["key", "generate"])
        .lines()
        .find_map(|line| {
            line.strip_prefix("Address:")
                .map(|rest| rest.trim().to_owned())
        })
        .expect("key generate prints the address");
    let root = sandbox.dir.path();
    sandbox.ok(root, &["init", "-q", "--bare", "upstream.git"]);
    let upstream = sandbox.path("upstream.git");
    let parked = sandbox.path("parked.git");
    let source = sandbox.path("source");
    sandbox.ok(
        &source,
        &[
            "remote",
            "add",
            "origin",
            &format!("torrent::file://{}", upstream.display()),
        ],
    );
    sandbox.ok(
        &source,
        &["config", "remote.origin.torrentAddress", &address],
    );
    sandbox.ok(&source, &["config", "remote.origin.torrentPublish", "true"]);
    sandbox.ok(&source, &["push", "-q", "origin", "main"]);
    let upstream_main = || {
        let output = sandbox.git(&upstream, &["rev-parse", "refs/heads/main"]);
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };

    // The upstream goes down; the push still lands, in the swarm.
    fs::rename(&upstream, &parked).expect("take the upstream down");
    let offline = sandbox.commit("OFFLINE");
    let stderr = sandbox.ok(&source, &["push", "origin", "main"]);
    assert!(
        stderr.contains("pushing to the swarm only") && stderr.contains("Signed manifest #2"),
        "the push is signed into the manifest instead:\n{stderr}"
    );
    let output = sandbox
        .command("git-torrent", &source)
        .arg("reconcile")
        .output()
        .expect("run reconcile");
    assert!(
        !output.status.success(),
        "reconcile fails while the upstream is still down"
    );

    // It comes back; reconcile replays the push.
    fs::rename(&parked, &upstream).expect("bring the upstream back");
    let out = sandbox.torrent(&["reconcile"]);
    assert!(out.contains("Reconciled refs/heads/main"), "{out}");
    assert_eq!(upstream_main(), offline, "the upstream caught up");
    assert!(
        sandbox
            .torrent(&["reconcile"])
            .contains("Nothing to reconcile"),
        "reconciled refs are cleared"
    );

    // If the upstream moved on meanwhile, reconcile refuses to overwrite it.
    fs::rename(&upstream, &parked).expect("take the upstream down again");
    sandbox.commit("SWARM-ONLY");
    sandbox.ok(&source, &["push", "-q", "origin", "main"]);
    fs::rename(&parked, &upstream).expect("bring the upstream back");
    sandbox.ok(
        root,
        &[
            "clone",
            "-q",
            &format!("file://{}", upstream.display()),
            "other",
        ],
    );
    let other = sandbox.path("other");
    fs::write(other.join("ELSEWHERE"), "x").expect("write file");
    sandbox.ok(&other, &["add", "ELSEWHERE"]);
    sandbox.ok(&other, &["commit", "-q", "-m", "elsewhere"]);
    sandbox.ok(&other, &["push", "-q", "origin", "main"]);
    let moved = upstream_main();

    let output = sandbox
        .command("git-torrent", &source)
        .arg("reconcile")
        .output()
        .expect("run reconcile");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("Fetch from it, merge"),
        "a conflict is reported, not overwritten:\n{stderr}"
    );
    assert_eq!(upstream_main(), moved, "the upstream's new commit survives");
    let output = sandbox
        .command("git-torrent", &source)
        .arg("reconcile")
        .output()
        .expect("run reconcile");
    assert!(
        !output.status.success(),
        "the rejected ref stays pending until it is resolved"
    );
}

#[test]
fn a_second_machine_with_the_same_key_cannot_publish() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    // One key file, two repositories: the key was copied to another machine.
    let address = sandbox
        .torrent(&["key", "generate"])
        .lines()
        .find_map(|line| {
            line.strip_prefix("Address:")
                .map(|rest| rest.trim().to_owned())
        })
        .expect("key generate prints the address");
    let (mut seeder, lines) = sandbox.seed();
    let peer = wait_for(&lines, "Seeding ")
        .rsplit(" on ")
        .next()
        .expect("address")
        .trim_end_matches('.')
        .to_owned();
    wait_for(&lines, "Published manifest #1");

    check(
        &sandbox.git_via(
            Some(&peer),
            sandbox.dir.path(),
            &[
                "clone",
                "-c",
                "remote.origin.torrentPublish=true",
                &address,
                "second",
            ],
        ),
        &["clone"],
    );
    let second = sandbox.path("second");
    fs::write(second.join("ELSEWHERE"), "x").expect("write file");
    sandbox.ok(&second, &["add", "ELSEWHERE"]);
    sandbox.ok(&second, &["commit", "-q", "-m", "elsewhere"]);
    let output = sandbox.git_via(Some(&peer), &second, &["push", "origin", "main"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains("signed on another machine"),
        "the second machine is refused before signing anything:\n{stderr}"
    );
    assert!(
        !second.join(".git/torrent/manifest").exists(),
        "nothing was signed on the second machine"
    );

    // The publisher itself is unaffected.
    let source = sandbox.path("source");
    sandbox.ok(&source, &["remote", "add", "swarm", &address]);
    sandbox.ok(&source, &["config", "remote.swarm.torrentPublish", "true"]);
    sandbox.commit("MORE");
    check(
        &sandbox.git_via(Some(&peer), &source, &["push", "swarm", "main"]),
        &["push"],
    );
    wait_for(&lines, "Published manifest #2");
    seeder.kill().expect("stop seeder");
    seeder.wait().expect("reap seeder");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_seeder_serves_several_repositories_and_republishes_others() {
    let sandbox = tokio::task::spawn_blocking(Sandbox::new)
        .await
        .expect("join");
    let root = sandbox.dir.path().to_path_buf();
    // Two repositories under one key, each with its own name.
    sandbox.commit("README");
    let address = sandbox
        .torrent(&["key", "generate"])
        .lines()
        .find_map(|line| {
            line.strip_prefix("Address:")
                .map(|rest| rest.trim().to_owned())
        })
        .expect("key generate prints the address");
    sandbox.torrent(&["sign", "--name", "alpha"]);
    sandbox.ok(&root, &["init", "-q", "beta"]);
    let beta = sandbox.path("beta");
    fs::write(beta.join("BETA"), "beta").expect("write file");
    sandbox.ok(&beta, &["add", "BETA"]);
    sandbox.ok(&beta, &["commit", "-q", "-m", "beta"]);
    let output = sandbox
        .command("git-torrent", &beta)
        .args(["sign", "--name", "beta"])
        .output()
        .expect("sign beta");
    check(&output, &["sign beta"]);

    // A third repository someone else published; the seeder only keeps it alive.
    let other = grt_core::PublisherKey::generate().expect("key");
    let mut manifest = grt_core::Manifest::new();
    manifest
        .insert_ref(
            "refs/heads/main".parse().expect("ref"),
            sandbox.rev_parse("source", "HEAD").parse().expect("id"),
        )
        .expect("insert");
    let signed = other
        .sign(&manifest, grt_core::Seq::new(1), b"")
        .expect("sign");
    let dht =
        grt_swarm::ManifestDht::with_bootstrap(&sandbox.bootstrap, std::net::Ipv4Addr::LOCALHOST)
            .expect("dht");
    dht.publish(&signed, None).await.expect("publish");
    let other_address = format!("torrent://{}", other.public_key());

    let (mut seeder, lines) = sandbox.seed_with(
        &root,
        &[
            "--repo",
            "source",
            "--repo",
            "beta",
            "--republish",
            &other_address,
        ],
    );
    let peer = tokio::task::block_in_place(|| wait_for(&lines, "Seeding "))
        .rsplit(" on ")
        .next()
        .expect("address")
        .trim_end_matches('.')
        .to_owned();
    tokio::task::block_in_place(|| {
        wait_for(
            &lines,
            &format!("Republished manifest #1 of {other_address}"),
        );
        wait_for(&lines, "Press Ctrl-C");
    });

    for (name, file) in [("alpha", "README"), ("beta", "BETA")] {
        let target = format!("clone-{name}");
        let output = tokio::task::block_in_place(|| {
            sandbox.git_via(
                Some(&peer),
                &root,
                &["clone", &format!("{address}/{name}"), &target],
            )
        });
        check(&output, &["clone", name]);
        assert!(
            sandbox.path(&target).join(file).exists(),
            "{name} was served by the shared seeder"
        );
    }

    // A push to one of them is published by the same process. (Starting the
    // seeder re-used #1, since nothing had changed since `sign`.)
    sandbox.ok(
        &beta,
        &["remote", "add", "swarm", &format!("{address}/beta")],
    );
    sandbox.ok(&beta, &["config", "remote.swarm.torrentPublish", "true"]);
    fs::write(beta.join("MORE"), "more").expect("write file");
    sandbox.ok(&beta, &["add", "MORE"]);
    sandbox.ok(&beta, &["commit", "-q", "-m", "more"]);
    let output = tokio::task::block_in_place(|| {
        sandbox.git_via(Some(&peer), &beta, &["push", "swarm", "main"])
    });
    check(&output, &["push beta"]);
    tokio::task::block_in_place(|| {
        wait_for(&lines, &format!("Published manifest #2 of {address}/beta"));
    });

    seeder.kill().expect("stop seeder");
    seeder.wait().expect("reap seeder");
}
