use std::io;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use crate::error::Error;
use crate::upstream::{ListedRef, PushResult, PushSpec, PushStatus, TransferOptions, Upstream};

/// An upstream reached by running `git` against its URL.
///
/// Child processes never inherit stdin or write to stdout: when this runs
/// inside the remote helper, both belong to the conversation with git.
#[derive(Debug, Clone)]
pub struct GitUpstream {
    url: String,
    git: PathBuf,
}

impl GitUpstream {
    /// Talks to `url` using the `git` found on `PATH`.
    pub fn new(url: impl Into<String>) -> Self {
        GitUpstream {
            url: url.into(),
            git: PathBuf::from("git"),
        }
    }

    /// The upstream URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    fn command(&self, subcommand: &str) -> Command {
        let mut command = Command::new(&self.git);
        command.arg(subcommand).stdin(Stdio::null());
        command
    }

    /// Runs `command`, capturing stdout. Stderr goes straight to the user so
    /// they see git's own errors, prompts and progress.
    fn capture(mut command: Command) -> Result<Output, Error> {
        let output = command
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()?;
        Ok(output)
    }
}

impl Upstream for GitUpstream {
    fn list(&self, _options: &TransferOptions) -> Result<Vec<ListedRef>, Error> {
        let mut command = self.command("ls-remote");
        command.args(["--symref", "--", &self.url]);
        let output = Self::capture(command)?;
        if !output.status.success() {
            return Err(Error::GitFailed {
                command: "ls-remote",
                status: output.status,
            });
        }
        parse_ls_remote(&String::from_utf8_lossy(&output.stdout))
    }

    fn fetch(&self, refs: &[String], options: &TransferOptions) -> Result<(), Error> {
        let mut command = self.command("fetch");
        command.args([
            "--no-tags",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
        ]);
        add_transfer_flags(&mut command, options);
        command.arg("--").arg(&self.url).args(refs);
        // Git's progress goes to stderr; anything on stdout is redirected there too.
        let status = command
            .stdout(io::stderr())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err(Error::GitFailed {
                command: "fetch",
                status,
            });
        }
        Ok(())
    }

    fn push(
        &self,
        specs: &[PushSpec],
        options: &TransferOptions,
    ) -> Result<Vec<PushResult>, Error> {
        let mut command = self.command("push");
        command.arg("--porcelain");
        // `--quiet` would also suppress the porcelain status we parse, so a
        // quiet push only silences progress.
        let options = if options.verbosity == 0 {
            &TransferOptions {
                verbosity: 1,
                progress: Some(false),
            }
        } else {
            options
        };
        add_transfer_flags(&mut command, options);
        command
            .arg("--")
            .arg(&self.url)
            .args(specs.iter().map(PushSpec::to_string));
        let output = Self::capture(command)?;
        let reported = parse_push_porcelain(&String::from_utf8_lossy(&output.stdout))?;
        // `git push` exits non-zero when any ref is rejected, so only treat it
        // as a total failure if nothing was reported.
        if reported.is_empty() && !output.status.success() {
            return Err(Error::GitFailed {
                command: "push",
                status: output.status,
            });
        }
        Ok(specs
            .iter()
            .map(|spec| {
                reported
                    .iter()
                    .find(|result| result.dst == spec.dst)
                    .cloned()
                    .unwrap_or_else(|| PushResult {
                        dst: spec.dst.clone(),
                        status: PushStatus::Rejected("no status reported by upstream".to_owned()),
                    })
            })
            .collect())
    }
}

fn add_transfer_flags(command: &mut Command, options: &TransferOptions) {
    match options.verbosity {
        0 => {
            command.arg("--quiet");
        }
        1 => {}
        _ => {
            command.arg("--verbose");
        }
    }
    match options.progress {
        Some(true) => {
            command.arg("--progress");
        }
        Some(false) => {
            command.arg("--no-progress");
        }
        None => {}
    }
}

/// Parses `git ls-remote --symref` output:
/// `ref: <target>\t<name>` for symbolic refs, `<oid>\t<name>` otherwise.
fn parse_ls_remote(stdout: &str) -> Result<Vec<ListedRef>, Error> {
    let mut refs = Vec::new();
    for line in stdout.lines() {
        let unexpected = || Error::UnexpectedOutput {
            command: "ls-remote",
            line: line.to_owned(),
        };
        let (value, name) = line.split_once('\t').ok_or_else(unexpected)?;
        if name.ends_with("^{}") {
            // Peeled tags are implied by the tag objects themselves.
            continue;
        }
        if let Some(target) = value.strip_prefix("ref: ") {
            refs.push(ListedRef::symbolic(name, target));
        } else if refs.iter().any(|known: &ListedRef| known.name == name) {
            // The symref line already described this ref; skip its resolved oid.
        } else {
            refs.push(ListedRef::object(
                name,
                value.parse().map_err(|_| unexpected())?,
            ));
        }
    }
    Ok(refs)
}

/// Parses `git push --porcelain` output: `<flag>\t<src>:<dst>\t<summary>`
/// lines between `To <url>` and `Done`.
fn parse_push_porcelain(stdout: &str) -> Result<Vec<PushResult>, Error> {
    let mut results = Vec::new();
    for line in stdout.lines() {
        if line.starts_with("To ") || line == "Done" || line.is_empty() {
            continue;
        }
        let unexpected = || Error::UnexpectedOutput {
            command: "push",
            line: line.to_owned(),
        };
        let mut fields = line.splitn(3, '\t');
        let (Some(flag), Some(refs), Some(summary)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(unexpected());
        };
        let (_, dst) = refs.split_once(':').ok_or_else(unexpected)?;
        let status = if flag == "!" {
            PushStatus::Rejected(rejection_reason(summary))
        } else {
            PushStatus::Ok
        };
        results.push(PushResult {
            dst: dst.to_owned(),
            status,
        });
    }
    Ok(results)
}

/// Extracts `non-fast-forward` from `[rejected] (non-fast-forward)`.
fn rejection_reason(summary: &str) -> String {
    summary
        .split_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or(summary)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::RefValue;

    const A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
    const B: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

    #[test]
    fn parses_ls_remote() {
        let stdout = format!(
            "ref: refs/heads/main\tHEAD\n{A}\tHEAD\n{A}\trefs/heads/main\n{B}\trefs/tags/v1\n{A}\trefs/tags/v1^{{}}\n"
        );
        let refs = parse_ls_remote(&stdout).unwrap();
        assert_eq!(
            refs,
            vec![
                ListedRef::symbolic("HEAD", "refs/heads/main"),
                ListedRef::object("refs/heads/main", A.parse().unwrap()),
                ListedRef::object("refs/tags/v1", B.parse().unwrap()),
            ],
            "HEAD stays symbolic and peeled tags are dropped"
        );
        assert!(
            matches!(
                parse_ls_remote(&format!("{A}\tHEAD\n")).unwrap()[0].value,
                RefValue::Object(_)
            ),
            "a detached HEAD is listed by object id"
        );
        assert!(
            parse_ls_remote("garbage\n").is_err(),
            "lines without a tab are rejected"
        );
    }

    #[test]
    fn parses_push_porcelain() {
        let stdout = "To file:///srv/up.git\n\
                      \trefs/heads/main:refs/heads/main\t1111111..2222222\n\
                      *\trefs/heads/new:refs/heads/new\t[new branch]\n\
                      !\trefs/heads/old:refs/heads/old\t[rejected] (non-fast-forward)\n\
                      Done\n";
        let results = parse_push_porcelain(stdout).unwrap();
        assert_eq!(
            results,
            vec![
                PushResult {
                    dst: "refs/heads/main".to_owned(),
                    status: PushStatus::Ok
                },
                PushResult {
                    dst: "refs/heads/new".to_owned(),
                    status: PushStatus::Ok
                },
                PushResult {
                    dst: "refs/heads/old".to_owned(),
                    status: PushStatus::Rejected("non-fast-forward".to_owned())
                },
            ]
        );
    }
}
