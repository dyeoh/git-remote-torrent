use std::io::Write;

use anyhow::bail;
use grt_core::RemoteUrl;
use grt_swarm::ManifestDht;

use crate::cli::RepublishArgs;

pub(crate) fn run(args: &RepublishArgs, out: &mut dyn Write) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let dht = ManifestDht::connect(&args.dht_bootstrap)?;
        let mut missing = Vec::new();
        for address in &args.addresses {
            let RemoteUrl::Swarm { key, salt } = address else {
                bail!("{address} is not a torrent://<key> address");
            };
            // resolve verifies the signature, so only genuine manifests spread.
            let Some(signed) = dht.resolve(key, salt).await? else {
                writeln!(out, "No manifest found for {address}.")?;
                missing.push(address.to_string());
                continue;
            };
            // The same signed item again: no key needed, and the nodes keep
            // it for another two hours or so.
            dht.publish(&signed, None).await?;
            writeln!(
                out,
                "Republished manifest #{} of {address}.",
                signed.seq().get()
            )?;
        }
        if !missing.is_empty() {
            bail!(
                "nothing to republish for {}; an expired manifest can only be published \
                 again by its publisher",
                missing.join(", ")
            );
        }
        Ok(())
    })
}
