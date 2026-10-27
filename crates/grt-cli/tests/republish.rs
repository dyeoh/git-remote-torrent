//! `git torrent republish` keeps someone else's manifest alive, with no key
//! and no repository, on a private DHT testnet.

use std::net::Ipv4Addr;
use std::process::Command;

use grt_core::{Manifest, PublisherKey, RemoteUrl, Seq};
use grt_swarm::ManifestDht;
use mainline::Testnet;
use tempfile::TempDir;

fn republish(bootstrap: &[String], args: &[String]) -> std::process::Output {
    let dir = TempDir::new().expect("create temp dir");
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-torrent"));
    command
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("GIT_TORRENT_HOME", dir.path().join("no-key-here"))
        .env("GIT_CONFIG_GLOBAL", dir.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("republish")
        .args(args);
    for node in bootstrap {
        command.args(["--dht-bootstrap", node]);
    }
    command.output().expect("run git-torrent")
}

#[tokio::test(flavor = "multi_thread")]
async fn anyone_can_republish_a_manifest() {
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(5).build().expect("testnet"))
        .await
        .expect("join");
    let key = PublisherKey::generate().expect("key");
    let mut manifest = Manifest::new();
    manifest
        .insert_ref(
            "refs/heads/main".parse().expect("ref"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
                .parse()
                .expect("id"),
        )
        .expect("insert");
    let signed = key.sign(&manifest, Seq::new(4), b"demo").expect("sign");
    let dht = ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST).expect("dht");
    dht.publish(&signed, None).await.expect("publish");
    let address = RemoteUrl::Swarm {
        key: key.public_key(),
        salt: b"demo".to_vec(),
    }
    .to_string();

    let bootstrap = testnet.bootstrap.clone();
    let output = {
        let address = address.clone();
        tokio::task::spawn_blocking(move || republish(&bootstrap, &[address]))
            .await
            .expect("join")
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("Republished manifest #4"),
        "republished without a key or repository:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved = dht
        .resolve(&key.public_key(), b"demo")
        .await
        .expect("resolve")
        .expect("still published");
    assert_eq!(resolved, signed, "the identical signed item, not a new one");

    let unknown = RemoteUrl::Swarm {
        key: PublisherKey::generate().expect("key").public_key(),
        salt: Vec::new(),
    }
    .to_string();
    let bootstrap = testnet.bootstrap.clone();
    let output = tokio::task::spawn_blocking(move || republish(&bootstrap, &[unknown]))
        .await
        .expect("join");
    assert!(
        !output.status.success()
            && String::from_utf8_lossy(&output.stderr)
                .contains("only be published again by its publisher"),
        "an address with nothing published cannot be revived"
    );

    // A copy revives an address the DHT no longer holds.
    let copy_dir = TempDir::new().expect("create temp dir");
    let key = PublisherKey::generate().expect("key");
    let old = key.sign(&manifest, Seq::new(1), b"").expect("sign");
    let copy = copy_dir.path().join("manifest");
    std::fs::write(&copy, old.encode()).expect("write copy");
    let args = vec![
        "--manifest".to_owned(),
        copy.to_str().expect("UTF-8").to_owned(),
    ];
    let bootstrap = testnet.bootstrap.clone();
    let revive = args.clone();
    let output = tokio::task::spawn_blocking(move || republish(&bootstrap, &revive))
        .await
        .expect("join");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success()
            && stdout.contains("Republished manifest #1 of")
            && stdout.contains("from its copy"),
        "revived from the copy:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let revived = dht
        .resolve(&key.public_key(), b"")
        .await
        .expect("resolve")
        .expect("published");
    assert_eq!(revived, old);

    // An old copy never replaces the newer manifest on the DHT.
    let newer = key.sign(&manifest, Seq::new(2), b"").expect("sign");
    dht.publish(&newer, Some(Seq::new(1)))
        .await
        .expect("publish #2");
    let bootstrap = testnet.bootstrap.clone();
    let output = tokio::task::spawn_blocking(move || republish(&bootstrap, &args))
        .await
        .expect("join");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success()
            && stdout.contains("Republished manifest #2 of")
            && !stdout.contains("from its copy"),
        "the newer published manifest wins:\n{stdout}"
    );
}
