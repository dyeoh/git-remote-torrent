use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, ensure};
use grt_git::Repo;

use crate::cli::ExportArgs;
use crate::sign;

pub(crate) fn run(args: &ExportArgs, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let signed = sign::load(&repo)?
        .context("no signed manifest yet; run `git torrent sign` or `git torrent seed` first")?;
    fs::create_dir_all(&args.dir).with_context(|| format!("creating {}", args.dir.display()))?;
    let packs = signed.manifest().packs();
    for &pack in packs {
        let source = sign::pack_path(&repo, pack);
        ensure!(
            source.exists(),
            "pack {pack} of the chain is missing ({}); it was signed on another machine",
            source.display()
        );
        let target = args.dir.join(format!("{pack}.pack"));
        // Content-addressed: a file already there with this name is this pack.
        if !target.exists() {
            fs::copy(&source, &target)
                .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
        }
    }
    writeln!(
        out,
        "Exported {} pack(s) of manifest #{} to {}.",
        packs.len(),
        signed.seq().get(),
        args.dir.display()
    )?;
    writeln!(
        out,
        "Upload the directory's files to a static host, then sign with \
         `--mirror <URL of the directory>`."
    )?;
    Ok(())
}
