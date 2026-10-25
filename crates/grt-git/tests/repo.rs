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

/// An empty bare repository in `format`, as a `Repo`.
fn bare(dir: &TempDir, name: &str, format: &str) -> Repo {
    git(
        dir.path(),
        &[
            "init",
            "-q",
            "--bare",
            &format!("--object-format={format}"),
            name,
        ],
    );
    Repo::discover(&dir.path().join(name)).expect("discover bare repo")
}

fn head_id(dir: &Path) -> ObjectId {
    git(dir, &["rev-parse", "HEAD"]).parse().expect("hex id")
}

#[test]
fn full_pack_round_trips_into_an_empty_repository() {
    for format in ["sha1", "sha256"] {
        let source = repo(format);
        let tip = head_id(source.path());
        let pack = source.path().join("full.pack");
        let count = Repo::discover(source.path())
            .expect("discover repo")
            .write_pack(&[tip], &[], &pack)
            .expect("write pack");
        assert_eq!(count, 3, "{format}: one commit, one tree, one blob");

        let target = TempDir::new().expect("create temp dir");
        let clone = bare(&target, "clone.git", format);
        assert!(
            !clone.contains(&tip).expect("query"),
            "{format}: empty repo lacks the tip"
        );
        clone.index_pack(&pack).expect("index full pack");
        assert!(
            clone.contains(&tip).expect("query"),
            "{format}: tip present after indexing"
        );
    }
}

#[test]
fn incremental_packs_apply_oldest_first() {
    let source = repo("sha1");
    let first = head_id(source.path());
    fs::write(source.path().join("README"), "changed\n").expect("write file");
    git(source.path(), &["commit", "-q", "-am", "two"]);
    let second = head_id(source.path());
    let source_repo = Repo::discover(source.path()).expect("discover repo");

    let full = source.path().join("1.pack");
    let next = source.path().join("2.pack");
    assert_eq!(
        source_repo
            .write_pack(&[first], &[], &full)
            .expect("full pack"),
        3
    );
    assert_eq!(
        source_repo
            .write_pack(&[second], &[first], &next)
            .expect("incremental pack"),
        3,
        "only the new commit, tree and blob"
    );

    let target = TempDir::new().expect("create temp dir");
    let alone = bare(&target, "alone.git", "sha1");
    assert!(
        alone.index_pack(&next).is_err(),
        "an incremental pack alone has a dangling parent link and must be rejected"
    );

    let chain = bare(&target, "chain.git", "sha1");
    chain.index_pack(&full).expect("index first pack");
    chain.index_pack(&next).expect("index second pack");
    assert!(
        chain.contains(&second).expect("query"),
        "chain yields the latest commit"
    );
}

#[test]
fn corrupt_pack_is_rejected() {
    let source = repo("sha1");
    let tip = head_id(source.path());
    let pack = source.path().join("full.pack");
    Repo::discover(source.path())
        .expect("discover repo")
        .write_pack(&[tip], &[], &pack)
        .expect("write pack");
    let mut bytes = fs::read(&pack).expect("read pack");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&pack, bytes).expect("rewrite pack");

    let target = TempDir::new().expect("create temp dir");
    let clone = bare(&target, "clone.git", "sha1");
    assert!(
        clone.index_pack(&pack).is_err(),
        "a flipped byte must fail indexing"
    );
    assert!(
        !clone.contains(&tip).expect("query"),
        "nothing from a bad pack is kept"
    );
}

#[test]
fn unknown_exclusions_are_ignored() {
    let source = repo("sha1");
    let tip = head_id(source.path());
    let unknown: ObjectId = "11".repeat(20).parse().expect("hex id");
    let count = Repo::discover(source.path())
        .expect("discover repo")
        .write_pack(&[tip], &[unknown], &source.path().join("p.pack"))
        .expect("an exclusion git has never seen cannot fail the pack");
    assert_eq!(count, 3);
}
