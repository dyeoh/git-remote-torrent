use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use grt_git::Repo;

use crate::cli::ReconcileArgs;
use crate::sign;

/// Written by the remote helper when a push reaches only the swarm: one
/// `<remote> <ref>` line per ref the upstream is missing.
const UNSYNCED_FILE: &str = "unsynced";

pub(crate) fn run(args: &ReconcileArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let path = repo.git_dir().join("torrent").join(UNSYNCED_FILE);
    let lines: BTreeSet<String> = match fs::read_to_string(&path) {
        Ok(contents) => contents.lines().map(str::to_owned).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let pending: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix(&format!("{} ", args.remote)))
        .collect();
    if pending.is_empty() {
        writeln!(out, "Nothing to reconcile for {}.", args.remote)?;
        return Ok(());
    }

    let url = repo
        .config(&format!("remote.{}.url", args.remote))?
        .with_context(|| format!("there is no remote {:?}", args.remote))?;
    let upstream = url
        .strip_prefix("torrent::")
        .with_context(|| format!("remote {:?} is not a torrent::<url> remote", args.remote))?;
    let signed = sign::load(&repo)?
        .context("no signed manifest; the pushes to reconcile were never signed here")?;

    // The manifest holds the swarm's current value of each ref; a ref no
    // longer in it was deleted.
    let specs: Vec<String> = pending
        .iter()
        .map(|name| {
            let name = name.parse()?;
            Ok(match signed.manifest().refs().get(&name) {
                Some(id) => format!("{id}:{name}"),
                None => format!(":{name}"),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(repo.git_dir())
        .args(["push", "--porcelain", upstream])
        .args(&specs)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()?;
    let reported = push_report(&String::from_utf8_lossy(&output.stdout));
    // No per-ref lines at all means the upstream never answered.
    if reported.is_empty() && !output.status.success() {
        bail!(
            "pushing to {upstream} failed ({}); is it back?",
            output.status
        );
    }

    let mut rejected = Vec::new();
    for name in &pending {
        if reported.get(*name) == Some(&true) {
            writeln!(out, "Reconciled {name}.")?;
        } else {
            rejected.push(*name);
        }
    }
    let remaining: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|line| {
            line.strip_prefix(&format!("{} ", args.remote))
                .is_none_or(|name| rejected.contains(&name))
        })
        .collect();
    if remaining.is_empty() {
        fs::remove_file(&path)?;
    } else {
        fs::write(&path, remaining.join("\n") + "\n")?;
    }
    if !rejected.is_empty() {
        bail!(
            "the upstream rejected {} because it changed while it was unreachable. \
             Fetch from it, merge, push again, and rerun `git torrent reconcile`.",
            rejected.join(", ")
        );
    }
    Ok(())
}

/// Each ref `git push --porcelain` reported on, and whether the upstream
/// accepted it (updated, deleted or already up to date). Lines look like
/// `<flag>\t<from>:<to>\t<summary>`, and the flag `!` means rejected.
fn push_report(porcelain: &str) -> BTreeMap<String, bool> {
    porcelain
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let flag = fields.next().filter(|flag| flag.len() == 1)?;
            let (_, to) = fields.next()?.rsplit_once(':')?;
            Some((to.to_owned(), flag != "!"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_push_output() {
        let porcelain = "To file:///up.git\n\
             *\trefs/heads/new:refs/heads/new\t[new branch]\n\
             \x20\tabc:refs/heads/main\t1111111..2222222\n\
             -\t:refs/heads/gone\t[deleted]\n\
             =\trefs/tags/v1:refs/tags/v1\t[up to date]\n\
             !\tdef:refs/heads/dev\t[rejected] (fetch first)\n\
             Done\n";
        let report = push_report(porcelain);
        let accepted: Vec<&str> = report
            .iter()
            .filter(|(_, accepted)| **accepted)
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            accepted,
            [
                "refs/heads/gone",
                "refs/heads/main",
                "refs/heads/new",
                "refs/tags/v1"
            ],
            "everything but `!` counts as reconciled"
        );
        assert_eq!(
            report.get("refs/heads/dev"),
            Some(&false),
            "rejections are reported"
        );
    }
}
