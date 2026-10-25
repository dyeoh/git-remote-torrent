//! `Repo` against real repositories in both of git's object formats.

use std::fs;
use std::path::Path;
use std::process::Command;

use grt_core::ObjectId;
use grt_git::Repo;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8")
        .trim()
        .to_owned()
}

/// A repository with one commit on `main` and a tag, in `format`.
fn repo(format: &str) -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "-b",
            "main",
            &format!("--object-format={format}"),
        ],
    );
    fs::write(dir.path().join("README"), "hi\n").expect("write file");
    git(dir.path(), &["add", "README"]);
    git(dir.path(), &["commit", "-q", "-m", "one"]);
    git(dir.path(), &["tag", "v1"]);
    dir
}

#[test]
fn lists_typed_refs_and_head() {
    let dir = repo("sha1");
    let repo = Repo::discover(dir.path()).expect("discover repo");
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    let refs = repo.refs(&["refs/heads", "refs/tags"]).expect("list refs");
    let names: Vec<&str> = refs.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["refs/heads/main", "refs/tags/v1"], "sorted by name");
    assert_eq!(
        refs[0].1,
        head.parse().expect("hex id"),
        "main points at HEAD"
    );
    assert!(
        matches!(refs[0].1, ObjectId::Sha1(_)),
        "default repos use SHA-1"
    );

    let target = repo.head_target().expect("read HEAD");
    assert_eq!(
        target.as_ref().map(grt_core::RefName::as_str),
        Some("refs/heads/main")
    );
}

#[test]
fn detached_head_has_no_target() {
    let dir = repo("sha1");
    git(dir.path(), &["checkout", "-q", "--detach"]);
    let repo = Repo::discover(dir.path()).expect("discover repo");
    assert_eq!(
        repo.head_target().expect("read HEAD"),
        None,
        "detached HEAD is not a ref"
    );
}

#[test]
fn sha256_repositories_yield_sha256_ids() {
    let dir = repo("sha256");
    let repo = Repo::discover(dir.path()).expect("discover repo");
    let refs = repo.refs(&["refs/heads"]).expect("list refs");
    assert!(
        matches!(refs[0].1, ObjectId::Sha256(_)),
        "a SHA-256 repository reports 32-byte ids"
    );
}
