//! Manifest format 1 test vectors (`tests/vectors/manifest-v1.txt`).
//!
//! The file is the frozen format: every valid vector must decode, encode to
//! the same bytes, and sign to the same signature; every invalid one must be
//! rejected for the stated reason. A change that breaks this test changes the
//! format. `print_vectors` regenerates the file's contents, for adding new
//! vectors only.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use grt_core::bencode::Value;
use grt_core::{InfoHash, InfoHashV2, Manifest, PublisherKey, Seq, SignedManifest};

const VECTORS: &str = include_str!("vectors/manifest-v1.txt");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "odd hex length: {text}");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex digit"))
        .collect()
}

/// The records of the vectors file, as field maps.
fn records() -> Vec<BTreeMap<String, String>> {
    VECTORS
        .split("\n\n")
        .map(|block| {
            block
                .lines()
                .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
                .map(|line| {
                    let (key, value) = line.split_once(':').expect("`field: value` line");
                    (key.to_owned(), value.trim().to_owned())
                })
                .collect::<BTreeMap<_, _>>()
        })
        .filter(|record| !record.is_empty())
        .collect()
}

#[test]
fn valid_vectors_decode_encode_and_sign_exactly() {
    let mut count = 0;
    for record in records().iter().filter(|record| record["kind"] == "valid") {
        let name = &record["name"];
        let seed: [u8; 32] = unhex(&record["seed"]).try_into().expect("32-byte seed");
        let key = PublisherKey::from_seed(seed);
        let salt = unhex(&record["salt"]);
        let seq = Seq::new(record["seq"].parse().expect("seq"));
        let value = unhex(&record["value"]);

        assert_eq!(
            key.public_key().to_string(),
            record["public-key"],
            "{name}: public key"
        );
        let manifest = Manifest::decode(&value).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(manifest.encode(), value, "{name}: canonical re-encoding");
        let signed = key.sign(&manifest, seq, &salt).expect("sign");
        assert_eq!(
            hex(signed.signature()),
            record["signature"],
            "{name}: signature"
        );
        let signature: [u8; 64] = unhex(&record["signature"]).try_into().expect("64 bytes");
        SignedManifest::verify(key.public_key(), &salt, seq, &value, &signature)
            .unwrap_or_else(|error| panic!("{name}: verify: {error}"));
        assert_eq!(
            hex(&signed.encode()),
            record["copy"],
            "{name}: manifest copy"
        );
        let copy = SignedManifest::decode(&unhex(&record["copy"]))
            .unwrap_or_else(|error| panic!("{name}: copy: {error}"));
        assert_eq!(copy.manifest(), &manifest, "{name}: copy decodes");
        count += 1;
    }
    assert!(count >= 3, "only {count} valid vectors");
}

#[test]
fn invalid_vectors_are_rejected_for_their_reason() {
    let mut count = 0;
    for record in records()
        .iter()
        .filter(|record| record["kind"] == "invalid")
    {
        let name = &record["name"];
        let error = Manifest::decode(&unhex(&record["value"]))
            .expect_err(&format!("{name}: must be rejected"))
            .to_string();
        assert!(
            error.starts_with(&record["error"]),
            "{name}: expected an error starting {:?}, got {error:?}",
            record["error"]
        );
        count += 1;
    }
    assert!(count >= 10, "only {count} invalid vectors");
}

/// Prints vectors in the file's format, from the builders below.
#[test]
#[ignore = "regenerates tests/vectors/manifest-v1.txt; run with --ignored --nocapture"]
fn print_vectors() {
    for (name, seed, salt, seq, manifest) in valid() {
        let key = PublisherKey::from_seed(seed);
        let signed = key.sign(&manifest, Seq::new(seq), salt).expect("sign");
        println!(
            "kind: valid\nname: {name}\nseed: {}\nsalt: {}\nseq: {seq}\npublic-key: {}\n\
             value: {}\nsignature: {}\ncopy: {}\n",
            hex(&seed),
            hex(salt),
            key.public_key(),
            hex(signed.value()),
            hex(signed.signature()),
            hex(&signed.encode())
        );
    }
    for (name, value) in invalid() {
        let error = Manifest::decode(&value).expect_err(name).to_string();
        println!(
            "kind: invalid\nname: {name}\nvalue: {}\nerror: {error}\n",
            hex(&value)
        );
    }
}

const MAIN: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
const TAG: &str = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";

type Valid = (&'static str, [u8; 32], &'static [u8], i64, Manifest);

fn valid() -> Vec<Valid> {
    let mut full = Manifest::new();
    full.insert_ref(
        "refs/heads/main".parse().expect("valid"),
        MAIN.parse().expect("valid"),
    )
    .expect("valid");
    full.insert_ref(
        "refs/tags/v1.0.0".parse().expect("valid"),
        TAG.parse().expect("valid"),
    )
    .expect("valid");
    full.set_head(Some("refs/heads/main".parse().expect("valid")))
        .expect("valid");
    full.push_pack(InfoHash::from([0x11; 20]));
    full.push_pack(InfoHash::from([0x22; 20]));
    full.set_packs_v2(Some(vec![
        InfoHashV2::from([0x33; 32]),
        InfoHashV2::from([0x44; 32]),
    ]))
    .expect("valid");
    full.set_upstream(Some("https://forge.example/group/project.git".to_owned()))
        .expect("valid");
    full.set_mirrors(vec!["https://packs.example/project".to_owned()])
        .expect("valid");
    full.set_signed_at(Some(1_767_225_600));

    let mut sha256 = Manifest::new();
    sha256
        .insert_ref(
            "refs/heads/main".parse().expect("valid"),
            "aa".repeat(32).parse().expect("valid"),
        )
        .expect("valid");
    sha256.push_pack(InfoHash::from([0x55; 20]));
    sha256
        .set_packs_v2(Some(vec![InfoHashV2::from([0x66; 32])]))
        .expect("valid");

    vec![
        ("empty", [1; 32], b"", 1, Manifest::new()),
        ("full", [2; 32], b"project", 7, full),
        ("sha256-repository", [3; 32], b"", 42, sha256),
    ]
}

fn dict(entries: &[(&[u8], Value)]) -> Vec<u8> {
    Value::Dict(
        entries
            .iter()
            .map(|(key, value)| (key.to_vec(), value.clone()))
            .collect(),
    )
    .encode()
}

fn bytes(value: &[u8]) -> Value {
    Value::Bytes(value.to_vec())
}

fn refs(entries: &[(&[u8], Vec<u8>)]) -> Value {
    Value::Dict(
        entries
            .iter()
            .map(|(name, id)| (name.to_vec(), Value::Bytes(id.clone())))
            .collect(),
    )
}

/// A valid manifest with one ref, no packs, and `extra` entries.
fn base(extra: &[(&[u8], Value)]) -> Vec<u8> {
    let mut entries: Vec<(&[u8], Value)> = vec![
        (b"packs", bytes(&[])),
        (b"refs", refs(&[(b"refs/heads/main", unhex(MAIN))])),
        (b"v", Value::Int(1)),
    ];
    entries.extend(extra.iter().cloned());
    dict(&entries)
}

fn invalid() -> Vec<(&'static str, Vec<u8>)> {
    let mut vectors = invalid_encodings();
    vectors.extend(invalid_fields());
    vectors
}

/// Values that aren't a canonical bencoded dictionary.
fn invalid_encodings() -> Vec<(&'static str, Vec<u8>)> {
    let mut trailing = base(&[]);
    trailing.push(b'x');
    vec![
        ("not-a-dictionary", b"le".to_vec()),
        ("unsorted-keys", b"d1:vi1e4:refsde5:packs0:e".to_vec()),
        ("trailing-bytes", trailing),
        ("leading-zero", b"d5:packs0:4:refsde1:vi01ee".to_vec()),
    ]
}

/// Canonical dictionaries whose fields break a rule.
fn invalid_fields() -> Vec<(&'static str, Vec<u8>)> {
    let main = unhex(MAIN);
    let one_ref = || refs(&[(b"refs/heads/main", main.clone())]);
    vec![
        (
            "version-2",
            dict(&[
                (b"packs", bytes(&[])),
                (b"refs", refs(&[])),
                (b"v", Value::Int(2)),
            ]),
        ),
        (
            "missing-refs",
            dict(&[(b"packs", bytes(&[])), (b"v", Value::Int(1))]),
        ),
        (
            "unknown-required-feature",
            base(&[(b"req", Value::List(vec![bytes(b"refs-file")]))]),
        ),
        (
            "pack-without-sha256",
            dict(&[
                (b"packs", bytes(&[0x11; 20])),
                (b"refs", one_ref()),
                (b"v", Value::Int(1)),
            ]),
        ),
        (
            "packs2-count-mismatch",
            dict(&[
                (b"packs", bytes(&[0x11; 20])),
                (b"packs2", bytes(&[0x33; 64])),
                (b"refs", one_ref()),
                (b"v", Value::Int(1)),
            ]),
        ),
        (
            "packs-not-multiple-of-20",
            dict(&[
                (b"packs", bytes(&[0x11; 19])),
                (b"refs", one_ref()),
                (b"v", Value::Int(1)),
            ]),
        ),
        (
            "dangling-head",
            base(&[(b"head", bytes(b"refs/heads/other"))]),
        ),
        (
            "ref-escapes-refs",
            dict(&[
                (b"packs", bytes(&[])),
                (
                    b"refs",
                    refs(&[(b"refs/heads/../../hooks/x", main.clone())]),
                ),
                (b"v", Value::Int(1)),
            ]),
        ),
        (
            "mixed-object-formats",
            dict(&[
                (b"packs", bytes(&[])),
                (
                    b"refs",
                    refs(&[
                        (b"refs/heads/a", main.clone()),
                        (b"refs/heads/b", vec![0xaa; 32]),
                    ]),
                ),
                (b"v", Value::Int(1)),
            ]),
        ),
        ("relative-upstream", base(&[(b"up", bytes(b"../project"))])),
        (
            "option-upstream",
            base(&[(b"up", bytes(b"--upload-pack=evil"))]),
        ),
        (
            "ftp-mirror",
            base(&[(b"ws", Value::List(vec![bytes(b"ftp://packs.example/p")]))]),
        ),
        ("negative-time", base(&[(b"t", Value::Int(-1))])),
    ]
}
