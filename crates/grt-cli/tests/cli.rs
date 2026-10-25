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
        resigned.contains("#2 with 2 ref(s) and 1 pack(s)") && !resigned.contains("Added pack"),
        "nothing new to pack, so the chain does not grow: {resigned}"
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
        out.contains("Signed manifest #2"),
        "same key and name increments seq: {out}"
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
