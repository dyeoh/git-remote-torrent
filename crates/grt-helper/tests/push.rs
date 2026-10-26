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
        let mut command = self.command("git-torrent", &self.path("source"));
        command.args(["seed", "--listen", "127.0.0.1:0", "--no-peer-dht"]);
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
