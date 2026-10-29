use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use grt_core::{PublicKey, PublisherKey};
use ssh_key::LineEnding;
use ssh_key::private::{Ed25519Keypair, Ed25519PrivateKey, KeypairData};
use ssh_key::public::Ed25519PublicKey;
use ssh_key::rand_core::OsRng;
use zeroize::Zeroizing;

use crate::cli::KeyCommand;

/// Global git config key holding the path of the signing key.
const SIGNING_KEY_CONFIG: &str = "torrent.signingKey";
const GENERATED_KEY_FILE: &str = "publisher_ed25519";
const GENERATED_KEY_COMMENT: &str = "git-torrent publisher";

pub(crate) fn run(command: KeyCommand, out: &mut dyn Write) -> anyhow::Result<()> {
    match command {
        KeyCommand::Generate { force, passphrase } => {
            let path = config_home()?.join(GENERATED_KEY_FILE);
            if path.exists() && !force {
                bail!(
                    "{} already exists; pass --force to replace it (anything signed with it \
                     can then no longer be updated)",
                    path.display()
                );
            }
            let passphrase = if passphrase {
                Some(new_passphrase()?)
            } else {
                None
            };
            let key = PublisherKey::generate()?;
            write_private_key(&path, &key, passphrase.as_deref().map(String::as_str))?;
            grt_git::set_global_config(SIGNING_KEY_CONFIG, &path.to_string_lossy())?;
            if passphrase.is_some() {
                writeln!(out, "Generated {} (encrypted).", path.display())?;
            } else {
                writeln!(
                    out,
                    "Generated {} (unencrypted; `--passphrase` encrypts it).",
                    path.display()
                )?;
            }
            print_public(out, &key.public_key())?;
        }
        KeyCommand::Use { path } => {
            let path =
                fs::canonicalize(&path).with_context(|| format!("reading {}", path.display()))?;
            let key = load_private_key(&path, ask_on_terminal)?;
            grt_git::set_global_config(SIGNING_KEY_CONFIG, &path.to_string_lossy())?;
            writeln!(out, "Signing with {}.", path.display())?;
            print_public(out, &key.public_key())?;
        }
        KeyCommand::Show => print_public(out, &signing_public_key()?)?,
    }
    Ok(())
}

/// Loads the configured signing key, prompting for its passphrase on the
/// terminal if needed.
pub(crate) fn signing_key() -> anyhow::Result<PublisherKey> {
    load_private_key(&signing_key_path()?, ask_on_terminal)
}

fn ask_on_terminal(path: &Path) -> anyhow::Result<Zeroizing<String>> {
    let prompt = format!("Passphrase for {}: ", path.display());
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}

/// A passphrase for a new key: typed twice on a terminal, or the first line
/// of stdin otherwise.
fn new_passphrase() -> anyhow::Result<Zeroizing<String>> {
    let passphrase = if io::stdin().is_terminal() {
        let first = Zeroizing::new(rpassword::prompt_password("New passphrase: ")?);
        let again = Zeroizing::new(rpassword::prompt_password("Same passphrase again: ")?);
        if first != again {
            bail!("the passphrases differ");
        }
        first
    } else {
        let mut line = Zeroizing::new(String::new());
        io::stdin().lock().read_line(&mut line)?;
        Zeroizing::new(line.trim_end_matches(['\r', '\n']).to_owned())
    };
    if passphrase.is_empty() {
        bail!("the passphrase is empty; leave out --passphrase for an unencrypted key");
    }
    Ok(passphrase)
}

/// Fails unless the configured signing key can be loaded with no one there to
/// type a passphrase, as a service must.
pub(crate) fn require_unattended_key() -> anyhow::Result<()> {
    let path = signing_key_path()?;
    if read_openssh(&path)?.is_encrypted() {
        bail!(
            "{} is encrypted, and a service can't ask for its passphrase. Sign with a \
             dedicated unencrypted key for this address (`git torrent key generate`), not \
             your personal SSH key, or seed with only --follow, which signs nothing",
            path.display()
        );
    }
    Ok(())
}

/// The configured signing key's public half; never prompts.
pub(crate) fn signing_public_key() -> anyhow::Result<PublicKey> {
    let path = signing_key_path()?;
    let key = read_openssh(&path)?;
    // OpenSSH stores the public key unencrypted, even in encrypted files.
    match key.public_key().key_data() {
        ssh_key::public::KeyData::Ed25519(Ed25519PublicKey(bytes)) => {
            Ok(PublicKey::try_from(bytes.as_slice())?)
        }
        _ => bail!("{} is not an Ed25519 key", path.display()),
    }
}

fn signing_key_path() -> anyhow::Result<PathBuf> {
    match grt_git::global_config(SIGNING_KEY_CONFIG)? {
        Some(path) => Ok(PathBuf::from(path)),
        None => bail!(
            "no signing key configured; run `git torrent key generate` or \
             `git torrent key use ~/.ssh/id_ed25519`"
        ),
    }
}

/// `$GIT_TORRENT_HOME`, else `$XDG_CONFIG_HOME/git-torrent`, else `~/.config/git-torrent`.
fn config_home() -> anyhow::Result<PathBuf> {
    if let Some(home) = std::env::var_os("GIT_TORRENT_HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config).join("git-torrent"));
    }
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".config").join("git-torrent")),
        None => bail!("cannot find a config directory: set GIT_TORRENT_HOME or HOME"),
    }
}

fn read_openssh(path: &Path) -> anyhow::Result<ssh_key::PrivateKey> {
    let pem = Zeroizing::new(
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    );
    ssh_key::PrivateKey::from_openssh(pem.as_bytes())
        .with_context(|| format!("{} is not an OpenSSH private key", path.display()))
}

/// Reads the key at `path`, asking `passphrase` for the passphrase if it is
/// encrypted.
fn load_private_key(
    path: &Path,
    passphrase: impl FnOnce(&Path) -> anyhow::Result<Zeroizing<String>>,
) -> anyhow::Result<PublisherKey> {
    let mut key = read_openssh(path)?;
    if key.is_encrypted() {
        let passphrase = passphrase(path)?;
        key = key
            .decrypt(passphrase.as_bytes())
            .context("wrong passphrase")?;
    }
    let KeypairData::Ed25519(pair) = key.key_data() else {
        bail!(
            "{} is a {} key; git-remote-torrent signs with Ed25519",
            path.display(),
            key.algorithm()
        );
    };
    let publisher = PublisherKey::from_seed(pair.private.to_bytes());
    if publisher.public_key().to_bytes() != pair.public.0 {
        bail!(
            "{} is corrupt: its public and private halves do not match",
            path.display()
        );
    }
    Ok(publisher)
}

fn write_private_key(
    path: &Path,
    key: &PublisherKey,
    passphrase: Option<&str>,
) -> anyhow::Result<()> {
    let pair = Ed25519Keypair {
        public: Ed25519PublicKey(key.public_key().to_bytes()),
        private: Ed25519PrivateKey::from_bytes(&key.to_seed()),
    };
    let mut private = ssh_key::PrivateKey::new(KeypairData::Ed25519(pair), GENERATED_KEY_COMMENT)?;
    if let Some(passphrase) = passphrase {
        private = private.encrypt(&mut OsRng, passphrase)?;
    }
    let pem = private.to_openssh(LineEnding::LF)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(pem.as_bytes())?;
    Ok(())
}

fn print_public(out: &mut dyn Write, key: &PublicKey) -> anyhow::Result<()> {
    writeln!(out, "Public key: {}", key.to_openssh(GENERATED_KEY_COMMENT))?;
    writeln!(out, "Address:    torrent://{key}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encrypted_key_needs_its_passphrase() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("key");
        let key = PublisherKey::generate().unwrap();
        write_private_key(&path, &key, Some("correct horse")).unwrap();
        assert!(read_openssh(&path).unwrap().is_encrypted());

        let given =
            |passphrase: &'static str| move |_: &Path| Ok(Zeroizing::new(passphrase.to_owned()));
        let loaded = load_private_key(&path, given("correct horse")).unwrap();
        assert_eq!(loaded.public_key(), key.public_key());
        let error = load_private_key(&path, given("wrong")).unwrap_err();
        assert!(error.to_string().contains("wrong passphrase"), "{error}");
    }

    #[test]
    fn an_unencrypted_key_never_asks() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("key");
        let key = PublisherKey::generate().unwrap();
        write_private_key(&path, &key, None).unwrap();
        let loaded = load_private_key(&path, |_| panic!("asked for a passphrase")).unwrap();
        assert_eq!(loaded.public_key(), key.public_key());
    }
}
