//! grt-swarm on localhost only: two torrent sessions and a private DHT testnet.

use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use grt_core::{InfoHash, Manifest, PublisherKey, Seq};
use grt_git::Repo;
use grt_swarm::{Error, ManifestDht, PackSwarm, SwarmConfig};
use mainline::Testnet;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
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

fn local_config() -> SwarmConfig {
    let mut config = SwarmConfig::default();
    config.listen = SocketAddr::from(([127, 0, 0, 1], 0));
    config.dht = false;
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn pack_travels_by_infohash_alone() {
    let dir = TempDir::new().expect("create temp dir");
    let source = dir.path().join("source");
    fs::create_dir(&source).expect("create source");
    git(&source, &["init", "-q", "-b", "main"]);
    fs::write(source.join("README"), "immortal\n").expect("write file");
    git(&source, &["add", "README"]);
    git(&source, &["commit", "-q", "-m", "one"]);
    let tip = git(&source, &["rev-parse", "HEAD"])
        .parse()
        .expect("hex id");

    let seed_dir = dir.path().join("seed");
    fs::create_dir(&seed_dir).expect("create seed dir");
    let pack = seed_dir.join("chain-0.pack");
    Repo::discover(&source)
        .expect("discover")
        .write_pack(&[tip], &[], &pack)
        .expect("write pack");

    let seeder = PackSwarm::start(&dir.path().join("seeder-state"), local_config())
        .await
        .expect("start seeder");
    let info_hash = seeder.seed(&pack).await.expect("seed pack");
    let seeder_addr = seeder.listen_addr().expect("seeder is listening");

    let leecher = PackSwarm::start(&dir.path().join("leecher-state"), local_config())
        .await
        .expect("start leecher");
    let fetched = leecher
        .fetch(
            info_hash,
            &[seeder_addr],
            &dir.path().join("fetched"),
            Duration::from_secs(60),
        )
        .await
        .expect("fetch pack");
    assert_eq!(
        fs::read(&fetched).expect("read fetched"),
        fs::read(&pack).expect("read original"),
        "fetched pack is byte-identical"
    );

    git(dir.path(), &["init", "-q", "--bare", "clone.git"]);
    let clone = Repo::discover(&dir.path().join("clone.git")).expect("discover clone");
    clone
        .index_pack(&fetched)
        .expect("git accepts the fetched pack");
    assert!(
        clone.contains(&tip).expect("query"),
        "the commit arrived through the swarm"
    );

    seeder.stop().await;
    leecher.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_with_no_way_to_find_peers_fails_fast() {
    let dir = TempDir::new().expect("create temp dir");
    let swarm = PackSwarm::start(dir.path(), local_config())
        .await
        .expect("start swarm");
    let result = swarm
        .fetch(
            InfoHash::from([7; 20]),
            &[],
            &dir.path().join("out"),
            Duration::from_secs(30),
        )
        .await;
    assert!(
        matches!(&result, Err(Error::Torrent(message)) if message.contains("no known way to resolve peers")),
        "no DHT, trackers or peers should fail at once, got {result:?}"
    );
    swarm.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_from_unresponsive_peer_times_out() {
    let dir = TempDir::new().expect("create temp dir");
    let swarm = PackSwarm::start(dir.path(), local_config())
        .await
        .expect("start swarm");

    // A peer that accepts connections and then never says a word.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("bind silent peer");
    let silent_addr = silent.local_addr().expect("silent peer address");
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in silent.incoming().flatten() {
            held.push(stream);
        }
    });

    let missing = InfoHash::from([7; 20]);
    let result = swarm
        .fetch(
            missing,
            &[silent_addr],
            &dir.path().join("out"),
            Duration::from_secs(2),
        )
        .await;
    assert!(
        matches!(result, Err(Error::Timeout(hash)) if hash == missing),
        "got {result:?}"
    );
    swarm.stop().await;
}

fn manifest(id: &str) -> Manifest {
    let mut manifest = Manifest::new();
    manifest
        .insert_ref(
            "refs/heads/main".parse().expect("ref"),
            id.parse().expect("hex id"),
        )
        .expect("insert");
    manifest
}

#[tokio::test(flavor = "multi_thread")]
async fn manifests_publish_resolve_and_detect_races() {
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(10).build().expect("testnet"))
        .await
        .expect("join");
    let node =
        || ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST).expect("dht node");
    let (publisher, reader) = (node(), node());
    let key = PublisherKey::from_seed([5; 32]);

    assert_eq!(
        reader
            .resolve(&key.public_key(), b"repo")
            .await
            .expect("resolve"),
        None,
        "nothing published yet"
    );

    let first = key
        .sign(&manifest(&"aa".repeat(20)), Seq::new(1), b"repo")
        .expect("sign");
    publisher
        .publish(&first, None)
        .await
        .expect("publish seq 1");
    let resolved = reader
        .resolve(&key.public_key(), b"repo")
        .await
        .expect("resolve");
    assert_eq!(
        resolved,
        Some(first),
        "another node resolves the verified manifest"
    );

    let second = key
        .sign(&manifest(&"bb".repeat(20)), Seq::new(2), b"repo")
        .expect("sign");
    publisher
        .publish(&second, Some(Seq::new(1)))
        .await
        .expect("publish seq 2 with cas 1");
    let resolved = reader
        .resolve(&key.public_key(), b"repo")
        .await
        .expect("resolve");
    assert_eq!(
        resolved.map(|m| m.seq()),
        Some(Seq::new(2)),
        "the update replaced seq 1"
    );

    let stale = key
        .sign(&manifest(&"cc".repeat(20)), Seq::new(3), b"repo")
        .expect("sign");
    let race = publisher.publish(&stale, Some(Seq::new(1))).await;
    assert!(
        matches!(race, Err(Error::Conflict(_))),
        "a publish based on seq 1 after seq 2 is a conflict, got {race:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn manifests_without_a_name_use_no_salt() {
    let testnet = tokio::task::spawn_blocking(|| Testnet::builder(10).build().expect("testnet"))
        .await
        .expect("join");
    let node =
        || ManifestDht::with_bootstrap(&testnet.bootstrap, Ipv4Addr::LOCALHOST).expect("dht node");
    let key = PublisherKey::from_seed([6; 32]);
    let signed = key
        .sign(&manifest(&"dd".repeat(20)), Seq::new(1), b"")
        .expect("sign");
    node()
        .publish(&signed, None)
        .await
        .expect("publish with an empty salt");
    let resolved = node()
        .resolve(&key.public_key(), b"")
        .await
        .expect("resolve");
    assert_eq!(
        resolved,
        Some(signed),
        "a key's only repository resolves with no salt"
    );
}
