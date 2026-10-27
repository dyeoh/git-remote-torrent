//! End-to-end: the `git-torrent` binary in an isolated git environment.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
        .expect("write sandbox gitconfig");
        let sandbox = Sandbox { dir };
        sandbox.git(&["init", "-q", "repo"], sandbox.dir.path());
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn repo(&self) -> PathBuf {
        self.path("repo")
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
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git prints UTF-8")
            .trim()
            .to_owned()
    }

    fn torrent(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_git-torrent"), &self.repo())
            .args(args)
            .output()
            .expect("run git-torrent")
    }

    /// Runs `git torrent` and returns stdout, panicking with stderr on failure.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.torrent(args);
        assert!(
            output.status.success(),
            "git torrent {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("git-torrent prints UTF-8")
    }

    fn fails(&self, args: &[&str]) -> String {
        let output = self.torrent(args);
        assert!(!output.status.success(), "git torrent {args:?} should fail");
        String::from_utf8(output.stderr).expect("git-torrent prints UTF-8")
    }

    fn commit(&self, file: &str) {
        fs::write(self.repo().join(file), file).expect("write work tree file");
        self.git(&["add", file], &self.repo());
        self.git(&["commit", "-q", "-m", file], &self.repo());
    }
}

#[test]
fn generate_key_and_refuse_to_overwrite() {
    let sandbox = Sandbox::new();
    let out = sandbox.ok(&["key", "generate"]);
    assert!(
        out.contains("Public key: ssh-ed25519 AAAA"),
        "prints the public key: {out}"
    );
    assert!(
        out.contains("Address:    torrent://"),
        "prints the address: {out}"
    );

    let key_path = sandbox.path("torrent-home/publisher_ed25519");
    assert_eq!(
        sandbox.git(
            &["config", "--global", "torrent.signingKey"],
            &sandbox.repo()
        ),
        key_path.to_string_lossy(),
        "the generated key becomes the signing key"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&key_path)
            .expect("key exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "private key must be readable only by the owner"
        );
    }

    let error = sandbox.fails(&["key", "generate"]);
    assert!(
        error.contains("--force"),
        "explains how to overwrite: {error}"
    );
    assert_eq!(
        sandbox.ok(&["key", "show"]),
        out.lines().skip(1).collect::<Vec<_>>().join("\n") + "\n"
    );
}

#[test]
fn use_existing_ssh_key() {
    let sandbox = Sandbox::new();
    let key_path = sandbox.path("id_ed25519");
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "alice@laptop", "-f"])
        .arg(&key_path)
        .status()
        .expect("run ssh-keygen");
    assert!(status.success(), "ssh-keygen failed");

    let out = sandbox.ok(&["key", "use", key_path.to_str().expect("UTF-8 path")]);
    let published = fs::read_to_string(key_path.with_extension("pub")).expect("read .pub");
    let base64 = published.split_whitespace().nth(1).expect("base64 field");
    assert!(out.contains(base64), "signing key is the SSH key: {out}");
}

#[test]
fn sign_status_and_trust_flow() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    sandbox.git(&["tag", "v0.1.0"], &sandbox.repo());
    sandbox.git(
        &[
            "remote",
            "add",
            "origin",
            "torrent::https://example.com/org/repo.git",
        ],
        &sandbox.repo(),
    );

    assert!(
        sandbox
            .fails(&["sign"])
            .contains("no signing key configured"),
        "signing without a key explains what to do"
    );
    assert!(sandbox.ok(&["status"]).contains("No signed manifest yet"));

    sandbox.ok(&["key", "generate"]);
    let out = sandbox.ok(&["sign"]);
    assert!(
        out.contains("Signed manifest #1 with 2 ref(s) and 1 pack(s), HEAD -> refs/heads/main."),
        "{out}"
    );
    assert!(
        out.contains("Added pack "),
        "the first manifest gets a full pack: {out}"
    );
    let resigned = sandbox.ok(&["sign"]);
    assert!(
        resigned.contains("Manifest #1 is up to date (2 ref(s), 1 pack(s))")
            && !resigned.contains("Added pack"),
        "nothing changed, so neither the chain nor the sequence number grows: {resigned}"
    );

    let status = sandbox.ok(&["status"]);
    assert!(
        status.contains("(NOT trusted)"),
        "own key is not trusted by default: {status}"
    );
    assert!(
        status.contains("Upstream:  https://example.com/org/repo.git"),
        "torrent:: is unwrapped: {status}"
    );
    assert!(
        status.contains("Manifest matches the local refs."),
        "{status}"
    );

    sandbox.ok(&["trust", "add", "--self"]);
    assert!(
        sandbox.ok(&["status"]).contains("(trusted)"),
        "trusting self is reflected"
    );

    sandbox.commit("CHANGES");
    let grown = sandbox.ok(&["sign"]);
    assert!(
        grown.contains("Added pack ") && grown.contains("and 2 pack(s)"),
        "a new commit adds a second pack: {grown}"
    );
    sandbox.commit("MORE");
    let status = sandbox.ok(&["status"]);
    assert!(
        status.contains("(local: "),
        "moved branch is flagged: {status}"
    );
    assert!(status.contains("run `git torrent sign`"), "{status}");

    let out = sandbox.ok(&["sign", "--name", "repo"]);
    assert!(
        out.contains("#1"),
        "a new name starts a new sequence: {out}"
    );
    assert!(out.contains("/repo"), "named address: {out}");
    let out = sandbox.ok(&["sign", "--name", "repo"]);
    assert!(
        out.contains("Manifest #1 is up to date"),
        "signing again with nothing changed keeps the sequence number: {out}"
    );
    sandbox.commit("NEWS");
    let out = sandbox.ok(&["sign", "--name", "repo"]);
    assert!(
        out.contains("Signed manifest #2"),
        "a change increments it: {out}"
    );
}

#[test]
fn tampered_manifest_is_rejected() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    sandbox.ok(&["key", "generate"]);
    sandbox.ok(&["sign"]);

    let path = sandbox.repo().join(".git/torrent/manifest");
    let mut bytes = fs::read(&path).expect("manifest cached");
    let last = bytes.len() - 3;
    bytes[last] ^= 1;
    fs::write(&path, bytes).expect("rewrite manifest");

    let error = sandbox.fails(&["status"]);
    assert!(
        error.contains("failed verification"),
        "tampering is detected: {error}"
    );
}

#[test]
fn trust_add_list_remove() {
    let sandbox = Sandbox::new();
    let hex = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";
    sandbox.ok(&["trust", "add", hex, "--label", "release key"]);
    assert!(
        sandbox
            .ok(&["trust", "add", hex])
            .contains("already trusted")
    );

    let list = sandbox.ok(&["trust", "list"]);
    assert_eq!(list, format!("{hex}  release key\n"));
    let stored =
        fs::read_to_string(sandbox.repo().join(".git/torrent/trusted-keys")).expect("store");
    assert!(
        stored.contains("ssh-ed25519 AAAA"),
        "store uses OpenSSH format: {stored}"
    );

    let line = stored
        .lines()
        .find(|line| line.starts_with("ssh-ed25519"))
        .expect("key line");
    sandbox.ok(&["trust", "remove", line]);
    assert!(
        sandbox
            .ok(&["trust", "list"])
            .contains("No trusted publishers")
    );
    assert!(
        sandbox
            .fails(&["trust", "remove", hex])
            .contains("is not trusted")
    );
    assert!(
        sandbox
            .fails(&["trust", "import", "http://example.com/alice"])
            .contains("not an https:// URL"),
        "forge imports require HTTPS"
    );
}

#[test]
fn mirrors_carry_over_and_export_lays_out_packs() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    sandbox.ok(&["key", "generate"]);

    let out = sandbox.ok(&["sign", "--mirror", "https://example.org/packs"]);
    assert!(
        out.contains("Mirror: https://example.org/packs"),
        "the mirror is signed into the manifest: {out}"
    );
    sandbox.commit("CHANGES");
    let out = sandbox.ok(&["sign"]);
    assert!(
        out.contains("Mirror: https://example.org/packs"),
        "re-signing without --mirror keeps the mirror: {out}"
    );
    let error = sandbox.fails(&["sign", "--mirror", "file:///etc"]);
    assert!(
        error.contains("ws"),
        "non-HTTP mirrors are refused: {error}"
    );

    let export = sandbox.path("export");
    let out = sandbox.ok(&["export", export.to_str().expect("UTF-8 path")]);
    assert!(out.contains("Exported 2 pack(s)"), "{out}");
    let mut files: Vec<String> = fs::read_dir(&export)
        .expect("export dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "one file per pack in the chain");
    assert!(
        files.iter().all(|name| name
            .strip_suffix(".pack")
            .is_some_and(|hash| hash.len() == 40)),
        "files are named <40-hex infohash>.pack: {files:?}"
    );
}

fn pack_dirs(sandbox: &Sandbox) -> usize {
    fs::read_dir(sandbox.repo().join(".git/torrent/packs"))
        .expect("packs dir")
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| !entry.file_name().to_string_lossy().starts_with("staging"))
        })
        .count()
}

#[test]
fn compact_folds_the_chain_into_one_complete_pack() {
    let sandbox = Sandbox::new();
    sandbox.ok(&["key", "generate"]);
    for file in ["A", "B", "C"] {
        sandbox.commit(file);
        sandbox.ok(&["sign"]);
    }
    assert!(sandbox.ok(&["status"]).contains("Packs:     3"));
    let main = sandbox.git(&["rev-parse", "main"], &sandbox.repo());
    // Compaction republishes the signed refs, not whatever is local now.
    sandbox.git(&["branch", "private"], &sandbox.repo());

    let out = sandbox.ok(&["compact"]);
    assert!(
        out.contains("Compacted the chain into pack") && out.contains("Signed manifest #4"),
        "{out}"
    );
    assert!(
        out.contains("3 old pack(s) stay"),
        "old packs are kept by default: {out}"
    );
    let status = sandbox.ok(&["status"]);
    assert!(status.contains("Packs:     1"), "{status}");
    assert!(
        status.contains("+ refs/heads/private  (not in manifest)"),
        "the new branch is local only: {status}"
    );

    // The single pack holds every commit: a fresh repository can index it alone.
    let pack = fs::read_dir(sandbox.repo().join(".git/torrent/packs"))
        .expect("packs dir")
        .map(|entry| entry.expect("entry").path().join("pack"))
        .find(|path| {
            let fresh = sandbox.path("fresh");
            let _ = fs::remove_dir_all(&fresh);
            sandbox.git(&["init", "-q", "--bare", "fresh"], sandbox.dir.path());
            Command::new("git")
                .args(["index-pack", "--stdin", "--strict"])
                .current_dir(&fresh)
                .stdin(fs::File::open(path).expect("open pack"))
                .stdout(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
                && Command::new("git")
                    .args(["cat-file", "-e", &main])
                    .current_dir(&fresh)
                    .status()
                    .is_ok_and(|status| status.success())
        });
    assert!(pack.is_some(), "one stored pack contains the whole history");

    sandbox.commit("D");
    sandbox.ok(&["sign"]);
    let out = sandbox.ok(&["compact", "--prune"]);
    assert!(out.contains("Pruned"), "{out}");
    assert_eq!(pack_dirs(&sandbox), 1, "only the compacted pack is left");
}

#[test]
fn sign_compacts_by_itself_when_the_manifest_is_full() {
    let sandbox = Sandbox::new();
    sandbox.commit("README");
    sandbox.ok(&["key", "generate"]);
    // Long ref names leave room for only a few pack ids.
    for i in 0..9 {
        sandbox.git(
            &["branch", &format!("a-fairly-long-branch-name-{i:02}")],
            &sandbox.repo(),
        );
    }
    let mut compacted = None;
    for round in 0..20 {
        sandbox.commit(&format!("file-{round}"));
        let out = sandbox.ok(&["sign"]);
        if out.contains("compacting the chain") {
            compacted = Some(out);
            break;
        }
    }
    let out = compacted.expect("the chain is compacted before the manifest overflows");
    assert!(
        out.contains("Compacted the chain into pack") && out.contains("and 1 pack(s)"),
        "{out}"
    );
}

#[test]
fn a_plain_sign_leaves_out_refs_that_exist_only_here() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo();
    sandbox.commit("README");
    sandbox.git(&["tag", "v1"], &repo);
    // A branch the remote has, and work that exists only on this machine.
    sandbox.git(&["branch", "feature"], &repo);
    sandbox.git(
        &["update-ref", "refs/remotes/origin/feature", "HEAD"],
        &repo,
    );
    sandbox.git(&["switch", "-q", "-c", "wip"], &repo);
    sandbox.commit("SECRET");
    sandbox.git(&["tag", "wip-tag"], &repo);
    sandbox.git(&["switch", "-q", "main"], &repo);
    sandbox.ok(&["key", "generate"]);

    let out = sandbox.ok(&["sign"]);
    assert!(
        out.contains("Publishing refs/heads/feature, refs/heads/main, refs/tags/v1.")
            && out.contains(
                "Not publishing refs that exist only here: refs/heads/wip, refs/tags/wip-tag"
            )
            && out.contains("Signed manifest #1 with 3 ref(s)"),
        "the current branch, the remote's branches and their tags only: {out}"
    );

    let out = sandbox.ok(&["sign", "--all"]);
    assert!(out.contains("Signed manifest #2 with 5 ref(s)"), "{out}");
    // Once published, refs stay published by a plain sign.
    let out = sandbox.ok(&["sign"]);
    assert!(out.contains("Manifest #2 is up to date (5 ref(s)"), "{out}");
}
