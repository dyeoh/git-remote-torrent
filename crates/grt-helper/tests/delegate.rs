//! End-to-end: real `git` driving `git-remote-torrent` in hybrid mode against
//! a local bare repository standing in for the upstream.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// An isolated git environment: no user or system config, the helper on PATH.
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
        Sandbox { dir }
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

    /// Runs git and panics with its stderr if it fails.
    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.git(cwd, args);
        assert!(
            output.status.success(),
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git prints UTF-8")
            .trim()
            .to_owned()
    }

    fn commit(&self, repo: &Path, file: &str, contents: &str) -> String {
        fs::write(repo.join(file), contents).expect("write work tree file");
        self.ok(repo, &["add", file]);
        self.ok(repo, &["commit", "-q", "-m", &format!("write {file}")]);
        self.ok(repo, &["rev-parse", "HEAD"])
    }
}

#[test]
fn clone_push_and_fetch_through_upstream() {
    let sandbox = Sandbox::new();
    let root = sandbox.dir.path();
    let upstream = sandbox.path("upstream.git");
    let remote = format!("torrent::file://{}", upstream.display());

    // An upstream with one commit, pushed directly (no helper involved).
    sandbox.ok(root, &["init", "-q", "--bare", "upstream.git"]);
    sandbox.ok(root, &["init", "-q", "seed"]);
    let seed = sandbox.path("seed");
    let first = sandbox.commit(&seed, "README", "hello\n");
    sandbox.ok(&seed, &["push", "-q", upstream.to_str().unwrap(), "main"]);

    // Clone through the helper.
    sandbox.ok(root, &["clone", "-q", &remote, "clone"]);
    let clone = sandbox.path("clone");
    assert_eq!(
        fs::read_to_string(clone.join("README")).unwrap(),
        "hello\n",
        "work tree checked out"
    );
    assert_eq!(
        sandbox.ok(&clone, &["rev-parse", "HEAD"]),
        first,
        "clone is at the upstream commit"
    );
    assert_eq!(
        sandbox.ok(&clone, &["symbolic-ref", "HEAD"]),
        "refs/heads/main",
        "HEAD follows the upstream's symbolic HEAD"
    );

    // Push a new commit through the helper.
    let second = sandbox.commit(&clone, "README", "hello again\n");
    sandbox.ok(&clone, &["push", "-q", "origin", "main"]);
    assert_eq!(
        sandbox.ok(&upstream, &["rev-parse", "main"]),
        second,
        "upstream received the push"
    );

    // A diverged push is rejected, and the upstream is untouched.
    sandbox.commit(&seed, "OTHER", "diverged\n");
    let rejected = sandbox.git(&seed, &["push", &remote, "main"]);
    assert!(
        !rejected.status.success(),
        "non-fast-forward push must fail"
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("rejected"),
        "git reports the rejection: {}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        sandbox.ok(&upstream, &["rev-parse", "main"]),
        second,
        "rejected push changed nothing"
    );

    // Fetch through the helper brings the new commit in.
    sandbox.ok(
        &seed,
        &["fetch", "-q", &remote, "main:refs/remotes/up/main"],
    );
    assert_eq!(
        sandbox.ok(&seed, &["rev-parse", "refs/remotes/up/main"]),
        second,
        "fetch updated the ref"
    );
}

#[test]
fn clone_of_empty_upstream() {
    let sandbox = Sandbox::new();
    let root = sandbox.dir.path();
    sandbox.ok(root, &["init", "-q", "--bare", "empty.git"]);
    let remote = format!("torrent::file://{}", sandbox.path("empty.git").display());
    sandbox.ok(root, &["clone", "-q", &remote, "clone"]);
    assert!(
        sandbox.path("clone/.git").is_dir(),
        "an empty repository clones to an empty repo"
    );
}

#[test]
fn unreachable_upstream_fails_cleanly() {
    let sandbox = Sandbox::new();
    let remote = format!("torrent::file://{}", sandbox.path("missing.git").display());
    let output = sandbox.git(sandbox.dir.path(), &["clone", "-q", &remote, "clone"]);
    assert!(
        !output.status.success(),
        "cloning a missing upstream must fail"
    );
}

#[test]
fn swarm_urls_are_not_supported_yet() {
    let sandbox = Sandbox::new();
    let key = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";
    let output = sandbox.git(
        sandbox.dir.path(),
        &["clone", &format!("torrent://{key}"), "clone"],
    );
    assert!(
        !output.status.success(),
        "pure P2P clone is not implemented yet"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not supported yet"),
        "the user is told why: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
