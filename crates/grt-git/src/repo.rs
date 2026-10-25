use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use grt_core::{ObjectId, RefName};

use crate::error::Error;

/// A local git repository.
#[derive(Debug, Clone)]
pub struct Repo {
    git_dir: PathBuf,
}

impl Repo {
    /// Finds the repository containing `dir`, like git itself does.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Failed`] if `dir` is not inside a git repository.
    pub fn discover(dir: &Path) -> Result<Repo, Error> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--absolute-git-dir"]);
        let git_dir = run(command, "rev-parse")?;
        Ok(Repo {
            git_dir: PathBuf::from(git_dir.trim_end()),
        })
    }

    /// The `.git` directory.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Lists refs matching `patterns` (for example `refs/heads`), sorted by
    /// name.
    ///
    /// Names and ids are validated here, at the boundary, with the same rules
    /// applied to refs from peers.
    ///
    /// # Errors
    ///
    /// Returns an error if git fails, prints an unparseable line, or reports
    /// a ref name or object id that fails validation.
    pub fn refs(&self, patterns: &[&str]) -> Result<Vec<(RefName, ObjectId)>, Error> {
        let mut command = self.git();
        command
            .args(["for-each-ref", "--format=%(objectname) %(refname)"])
            .args(patterns);
        let stdout = run(command, "for-each-ref")?;
        stdout
            .lines()
            .map(|line| {
                let (id, name) = line
                    .split_once(' ')
                    .ok_or_else(|| Error::UnexpectedOutput {
                        command: "for-each-ref".to_owned(),
                        line: line.to_owned(),
                    })?;
                Ok((name.parse()?, id.parse()?))
            })
            .collect()
    }

    /// The ref `HEAD` points to, or `None` if `HEAD` is detached.
    ///
    /// # Errors
    ///
    /// Returns an error if git cannot be run or `HEAD` points at an invalid
    /// ref name.
    pub fn head_target(&self) -> Result<Option<RefName>, Error> {
        let mut command = self.git();
        command.args(["symbolic-ref", "--quiet", "HEAD"]);
        let output = command.stdin(Stdio::null()).output()?;
        // Exit status 1 with --quiet means HEAD is detached.
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        check(&output, "symbolic-ref")?;
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim_end().parse()?,
        ))
    }

    /// Reads a config value, as `git config --get` sees it from this repository.
    ///
    /// # Errors
    ///
    /// Returns an error if git cannot be run or the config is invalid.
    pub fn config(&self, key: &str) -> Result<Option<String>, Error> {
        let mut command = self.git();
        command.args(["config", "--get", key]);
        config_get(command)
    }

    fn git(&self) -> Command {
        let mut command = Command::new("git");
        command.arg("--git-dir").arg(&self.git_dir);
        command
    }
}

/// Reads a value from the user's global git config.
///
/// # Errors
///
/// Returns an error if git cannot be run or the config is invalid.
pub fn global_config(key: &str) -> Result<Option<String>, Error> {
    let mut command = Command::new("git");
    command.args(["config", "--global", "--get", key]);
    config_get(command)
}

/// Writes a value to the user's global git config.
///
/// # Errors
///
/// Returns an error if git fails.
pub fn set_global_config(key: &str, value: &str) -> Result<(), Error> {
    let mut command = Command::new("git");
    command.args(["config", "--global", key, value]);
    run(command, "config").map(drop)
}

fn config_get(mut command: Command) -> Result<Option<String>, Error> {
    let output = command.stdin(Stdio::null()).output()?;
    // `git config --get` exits 1 when the key is unset.
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    check(&output, "config")?;
    Ok(Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned(),
    ))
}

fn run(mut command: Command, name: &str) -> Result<String, Error> {
    let output = command.stdin(Stdio::null()).output()?;
    check(&output, name)?;
    String::from_utf8(output.stdout).map_err(|error| Error::UnexpectedOutput {
        command: name.to_owned(),
        line: String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

fn check(output: &std::process::Output, name: &str) -> Result<(), Error> {
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Failed {
        command: name.to_owned(),
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}
