use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;

use grt_core::ObjectId;

use crate::error::Error;
use crate::repo::{Repo, check};

impl Repo {
    /// Writes a self-contained (non-thin) pack to `out` holding every object
    /// reachable from `tips` but not from `exclude`, and returns its object
    /// count.
    ///
    /// With an empty `exclude` this is a full pack. Excluding the tips of the
    /// previous manifest gives the next pack of the chain (ADR-0004). Ids in
    /// `exclude` that this repository does not have are skipped, since git
    /// cannot exclude history it has never seen.
    ///
    /// # Errors
    ///
    /// Returns an error if `out` cannot be created or `git pack-objects`
    /// fails, for example because a tip is missing.
    pub fn write_pack(
        &self,
        tips: &[ObjectId],
        exclude: &[ObjectId],
        out: &Path,
    ) -> Result<u32, Error> {
        let mut known_exclusions = Vec::new();
        for id in exclude {
            if self.contains(id)? {
                known_exclusions.push(id);
            }
        }
        let revs: String = tips
            .iter()
            .map(|tip| format!("{tip}\n"))
            .chain(known_exclusions.iter().map(|id| format!("^{id}\n")))
            .collect();

        let mut command = self.git();
        command
            .args(["pack-objects", "--revs", "--stdout", "-q"])
            .stdin(Stdio::piped())
            .stdout(File::create(out)?)
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("pack-objects has no stdin"))?;
        stdin.write_all(revs.as_bytes())?;
        // Close stdin so pack-objects sees the end of the rev list.
        drop(stdin);
        let output = child.wait_with_output()?;
        check(&output, "pack-objects")?;
        pack_object_count(out)
    }

    /// Adds the pack at `pack` to this repository's object store with
    /// `git index-pack --stdin --strict`, which checks every object's id and
    /// that the pack's links resolve.
    ///
    /// Packs of a chain must be indexed oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the pack is corrupt, holds a malformed object, or
    /// refers to objects this repository does not have.
    pub fn index_pack(&self, pack: &Path) -> Result<(), Error> {
        let mut command = self.git();
        command
            .args(["index-pack", "--stdin", "--strict"])
            .stdin(File::open(pack)?)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output()?;
        check(&output, "index-pack")
    }

    /// Returns true if the object `id` is in this repository.
    ///
    /// # Errors
    ///
    /// Returns an error if git cannot be run.
    pub fn contains(&self, id: &ObjectId) -> Result<bool, Error> {
        let mut command = self.git();
        command
            .args(["cat-file", "-e", &id.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Ok(command.status()?.success())
    }
}

/// Reads the object count from a pack header: `PACK`, a 4-byte version, then
/// a 4-byte big-endian count.
fn pack_object_count(pack: &Path) -> Result<u32, Error> {
    let mut header = [0u8; 12];
    File::open(pack)?.read_exact(&mut header)?;
    if &header[..4] != b"PACK" {
        return Err(Error::UnexpectedOutput {
            command: "pack-objects".to_owned(),
            line: "output does not start with a PACK header".to_owned(),
        });
    }
    Ok(u32::from_be_bytes([
        header[8], header[9], header[10], header[11],
    ]))
}
