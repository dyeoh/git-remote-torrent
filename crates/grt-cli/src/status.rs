use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use grt_core::{ObjectId, RefName, RemoteUrl};
use grt_git::Repo;

use crate::{sign, trust};

pub(crate) fn run(cwd: &Path, out: &mut dyn Write) -> anyhow::Result<()> {
    let repo = Repo::discover(cwd)?;
    let Some(signed) = sign::load(&repo)? else {
        writeln!(out, "No signed manifest yet. Run `git torrent sign`.")?;
        return Ok(());
    };
    let manifest = signed.manifest();
    let trusted = trust::load(&repo)?.contains(&signed.public_key());
    let address = RemoteUrl::Swarm {
        key: signed.public_key(),
        salt: signed.salt().to_vec(),
    };

    writeln!(out, "Address:   {address}")?;
    writeln!(
        out,
        "Publisher: {} ({})",
        signed.public_key(),
        if trusted { "trusted" } else { "NOT trusted" }
    )?;
    writeln!(out, "Sequence:  {}", signed.seq().get())?;
    writeln!(
        out,
        "Upstream:  {}",
        manifest.upstream().unwrap_or("(none)")
    )?;
    writeln!(
        out,
        "HEAD:      {}",
        manifest
            .head()
            .map_or("(detached or unset)".to_owned(), ToString::to_string)
    )?;

    let local: BTreeMap<RefName, ObjectId> = repo.refs(sign::PUBLISHED_REFS)?.into_iter().collect();
    let mut in_sync = true;
    writeln!(out, "Refs:")?;
    for (name, id) in manifest.refs() {
        let note = match local.get(name) {
            Some(local_id) if local_id == id => String::new(),
            Some(local_id) => format!("  (local: {local_id})"),
            None => "  (missing locally)".to_owned(),
        };
        in_sync &= note.is_empty();
        writeln!(out, "  {id} {name}{note}")?;
    }
    for name in local
        .keys()
        .filter(|name| !manifest.refs().contains_key(*name))
    {
        in_sync = false;
        writeln!(out, "  + {name}  (not in manifest)")?;
    }
    if in_sync {
        writeln!(out, "Manifest matches the local refs.")?;
    } else {
        writeln!(
            out,
            "Local refs have changed; run `git torrent sign` to publish them."
        )?;
    }
    Ok(())
}
