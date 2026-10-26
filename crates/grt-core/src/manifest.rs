use std::collections::BTreeMap;

use crate::bencode::Value;
use crate::error::Error;
use crate::info_hash::{InfoHash, InfoHashV2};
use crate::object_id::ObjectId;
use crate::ref_name::RefName;

/// The manifest format version this build writes and reads.
///
/// Readers reject other versions and ignore unknown keys; see
/// `docs/reference/versioning.md`.
pub const MANIFEST_VERSION: i64 = 1;

// Short keys: the whole manifest must fit in a 1000-byte BEP 44 value.
const KEY_VERSION: &[u8] = b"v";
const KEY_HEAD: &[u8] = b"head";
const KEY_REFS: &[u8] = b"refs";
const KEY_PACKS: &[u8] = b"packs";
const KEY_PACKS_V2: &[u8] = b"packs2";
const KEY_UPSTREAM: &[u8] = b"up";
const KEY_MIRRORS: &[u8] = b"ws";

/// Longest mirror base URL accepted, in bytes.
pub const MAX_MIRROR_LEN: usize = 200;

/// A repository's published state: its refs and the packs that hold their objects.
///
/// Encoded as a bencode dictionary:
///
/// | key | value |
/// |-----|-------|
/// | `v` | format version ([`MANIFEST_VERSION`]) |
/// | `refs` | dictionary of ref name → raw object id |
/// | `packs` | concatenated 20-byte BitTorrent v1 pack infohashes, oldest first |
/// | `packs2` | optional; concatenated 32-byte v2 infohashes of the same packs, same order |
/// | `head` | optional ref name that `HEAD` points to |
/// | `up` | optional canonical upstream URL |
/// | `ws` | optional list of HTTP mirror base URLs serving `<infohash>.pack` |
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Manifest {
    head: Option<RefName>,
    refs: BTreeMap<RefName, ObjectId>,
    packs: Vec<InfoHash>,
    packs_v2: Option<Vec<InfoHashV2>>,
    upstream: Option<String>,
    mirrors: Vec<String>,
}

impl Manifest {
    /// Creates an empty manifest.
    pub fn new() -> Self {
        Manifest::default()
    }

    /// The ref `HEAD` points to, if any.
    pub fn head(&self) -> Option<&RefName> {
        self.head.as_ref()
    }

    /// Sets the ref `HEAD` points to.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidField`] if `head` is not one of the manifest's refs.
    pub fn set_head(&mut self, head: Option<RefName>) -> Result<(), Error> {
        if head
            .as_ref()
            .is_some_and(|name| !self.refs.contains_key(name))
        {
            return Err(Error::InvalidField {
                field: "head",
                problem: "not one of the manifest's refs",
            });
        }
        self.head = head;
        Ok(())
    }

    /// All refs, sorted by name.
    pub fn refs(&self) -> &BTreeMap<RefName, ObjectId> {
        &self.refs
    }

    /// Adds or updates a ref.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MixedObjectFormats`] if `id` uses a different object
    /// format (SHA-1 vs SHA-256) from the refs already present.
    pub fn insert_ref(&mut self, name: RefName, id: ObjectId) -> Result<(), Error> {
        if self
            .refs
            .values()
            .next()
            .is_some_and(|existing| !existing.same_format(&id))
        {
            return Err(Error::MixedObjectFormats);
        }
        self.refs.insert(name, id);
        Ok(())
    }

    /// Pack infohashes (BitTorrent v1), oldest first.
    pub fn packs(&self) -> &[InfoHash] {
        &self.packs
    }

    /// Appends a pack to the chain.
    ///
    /// This clears [`Manifest::packs_v2`], which must list exactly the same
    /// packs; set it again afterwards if the new pack has a v2 torrent too.
    pub fn push_pack(&mut self, pack: InfoHash) {
        self.packs.push(pack);
        self.packs_v2 = None;
    }

    /// BitTorrent v2 infohashes for the same packs as [`Manifest::packs`], in
    /// the same order, if the publisher provides them.
    pub fn packs_v2(&self) -> Option<&[InfoHashV2]> {
        self.packs_v2.as_deref()
    }

    /// Sets the v2 infohashes of the packs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidField`] if `packs_v2` does not have one entry
    /// per pack in [`Manifest::packs`].
    pub fn set_packs_v2(&mut self, packs_v2: Option<Vec<InfoHashV2>>) -> Result<(), Error> {
        if packs_v2
            .as_ref()
            .is_some_and(|v2| v2.len() != self.packs.len())
        {
            return Err(invalid("packs2", "not one entry per pack"));
        }
        self.packs_v2 = packs_v2;
        Ok(())
    }

    /// The canonical upstream URL, if the repository has one.
    pub fn upstream(&self) -> Option<&str> {
        self.upstream.as_deref()
    }

    /// Sets the canonical upstream URL.
    pub fn set_upstream(&mut self, upstream: Option<String>) {
        self.upstream = upstream;
    }

    /// HTTP mirrors that serve this manifest's packs as
    /// `<base>/<infohash>.pack`. Downloads from them are checked against the
    /// infohash, so a mirror needs no trust.
    pub fn mirrors(&self) -> &[String] {
        &self.mirrors
    }

    /// Sets the HTTP mirror base URLs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidField`] if a URL is not `http://` or
    /// `https://`, is longer than [`MAX_MIRROR_LEN`], or contains whitespace
    /// or control characters.
    pub fn set_mirrors(&mut self, mirrors: Vec<String>) -> Result<(), Error> {
        for mirror in &mirrors {
            check_mirror(mirror)?;
        }
        self.mirrors = mirrors;
        Ok(())
    }

    /// Encodes the manifest as canonical bencode.
    pub fn encode(&self) -> Vec<u8> {
        let refs = self
            .refs
            .iter()
            .map(|(name, id)| {
                (
                    name.as_str().as_bytes().to_vec(),
                    Value::Bytes(id.as_bytes().to_vec()),
                )
            })
            .collect();
        let packs = self
            .packs
            .iter()
            .flat_map(InfoHash::as_bytes)
            .copied()
            .collect();

        let mut dict = BTreeMap::new();
        dict.insert(KEY_VERSION.to_vec(), Value::Int(MANIFEST_VERSION));
        dict.insert(KEY_REFS.to_vec(), Value::Dict(refs));
        dict.insert(KEY_PACKS.to_vec(), Value::Bytes(packs));
        if let Some(packs_v2) = &self.packs_v2 {
            let packs_v2 = packs_v2
                .iter()
                .flat_map(InfoHashV2::as_bytes)
                .copied()
                .collect();
            dict.insert(KEY_PACKS_V2.to_vec(), Value::Bytes(packs_v2));
        }
        if let Some(head) = &self.head {
            dict.insert(
                KEY_HEAD.to_vec(),
                Value::Bytes(head.as_str().as_bytes().to_vec()),
            );
        }
        if let Some(upstream) = &self.upstream {
            dict.insert(
                KEY_UPSTREAM.to_vec(),
                Value::Bytes(upstream.as_bytes().to_vec()),
            );
        }
        if !self.mirrors.is_empty() {
            let mirrors = self
                .mirrors
                .iter()
                .map(|mirror| Value::Bytes(mirror.as_bytes().to_vec()))
                .collect();
            dict.insert(KEY_MIRRORS.to_vec(), Value::List(mirrors));
        }
        Value::Dict(dict).encode()
    }

    /// Decodes and validates a manifest. Unknown keys are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Bencode`] for malformed input,
    /// [`Error::UnsupportedVersion`] for another format version, and
    /// [`Error::InvalidField`], [`Error::InvalidRefName`],
    /// [`Error::InvalidLength`] or [`Error::MixedObjectFormats`] when a field
    /// fails validation.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let value = Value::decode(bytes)?;
        let dict = value.as_dict().ok_or(Error::InvalidField {
            field: "manifest",
            problem: "not a dictionary",
        })?;

        let version = required(dict, KEY_VERSION, "v")?
            .as_int()
            .ok_or(invalid("v", "not an integer"))?;
        if version != MANIFEST_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }

        let mut manifest = Manifest::new();
        let refs = required(dict, KEY_REFS, "refs")?
            .as_dict()
            .ok_or(invalid("refs", "not a dictionary"))?;
        for (name, id) in refs {
            let id = id
                .as_bytes()
                .ok_or(invalid("refs", "has a non-string object id"))?;
            manifest.insert_ref(RefName::try_from(name.as_slice())?, ObjectId::try_from(id)?)?;
        }

        let packs = required(dict, KEY_PACKS, "packs")?
            .as_bytes()
            .ok_or(invalid("packs", "not a byte string"))?;
        let chunks = packs.chunks_exact(InfoHash::LEN);
        if !chunks.remainder().is_empty() {
            return Err(invalid("packs", "not a multiple of 20 bytes"));
        }
        for chunk in chunks {
            manifest.push_pack(InfoHash::try_from(chunk)?);
        }

        if let Some(packs_v2) = dict.get(KEY_PACKS_V2) {
            let packs_v2 = packs_v2
                .as_bytes()
                .ok_or(invalid("packs2", "not a byte string"))?;
            let chunks = packs_v2.chunks_exact(InfoHashV2::LEN);
            if !chunks.remainder().is_empty() {
                return Err(invalid("packs2", "not a multiple of 32 bytes"));
            }
            let packs_v2 = chunks
                .map(InfoHashV2::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            manifest.set_packs_v2(Some(packs_v2))?;
        }

        if let Some(head) = dict.get(KEY_HEAD) {
            let head = head
                .as_bytes()
                .ok_or(invalid("head", "not a byte string"))?;
            manifest.set_head(Some(RefName::try_from(head)?))?;
        }

        if let Some(upstream) = dict.get(KEY_UPSTREAM) {
            let upstream = upstream
                .as_bytes()
                .ok_or(invalid("up", "not a byte string"))?;
            let upstream = std::str::from_utf8(upstream).map_err(|_| invalid("up", "not UTF-8"))?;
            manifest.set_upstream(Some(upstream.to_owned()));
        }

        if let Some(mirrors) = dict.get(KEY_MIRRORS) {
            let mirrors = mirrors
                .as_list()
                .ok_or(invalid("ws", "not a list"))?
                .iter()
                .map(|mirror| {
                    let mirror = mirror
                        .as_bytes()
                        .ok_or(invalid("ws", "has a non-string entry"))?;
                    std::str::from_utf8(mirror)
                        .map(str::to_owned)
                        .map_err(|_| invalid("ws", "not UTF-8"))
                })
                .collect::<Result<Vec<String>, Error>>()?;
            manifest.set_mirrors(mirrors)?;
        }

        Ok(manifest)
    }
}

fn check_mirror(mirror: &str) -> Result<(), Error> {
    if !(mirror.starts_with("https://") || mirror.starts_with("http://")) {
        return Err(invalid("ws", "a mirror is not an http:// or https:// URL"));
    }
    if mirror.len() > MAX_MIRROR_LEN {
        return Err(invalid("ws", "a mirror URL is longer than 200 bytes"));
    }
    if mirror.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid("ws", "a mirror URL contains whitespace"));
    }
    Ok(())
}

fn required<'a>(
    dict: &'a BTreeMap<Vec<u8>, Value>,
    key: &[u8],
    field: &'static str,
) -> Result<&'a Value, Error> {
    dict.get(key).ok_or(invalid(field, "missing"))
}

fn invalid(field: &'static str, problem: &'static str) -> Error {
    Error::InvalidField { field, problem }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        let mut manifest = Manifest::new();
        let main: RefName = "refs/heads/main".parse().unwrap();
        manifest
            .insert_ref(
                main.clone(),
                "ce013625030ba8dba906f756967f9e9ca394464a".parse().unwrap(),
            )
            .unwrap();
        manifest
            .insert_ref(
                "refs/tags/v0.1.0".parse().unwrap(),
                "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391".parse().unwrap(),
            )
            .unwrap();
        manifest.set_head(Some(main)).unwrap();
        manifest.push_pack(InfoHash::from([1; 20]));
        manifest.push_pack(InfoHash::from([2; 20]));
        manifest.set_upstream(Some("https://github.com/org/repo.git".to_owned()));
        manifest
            .set_mirrors(vec!["https://example.org/packs".to_owned()])
            .unwrap();
        manifest
    }

    #[test]
    fn round_trips() {
        let manifest = sample();
        assert_eq!(
            Manifest::decode(&manifest.encode()),
            Ok(manifest),
            "decode(encode(m)) == m"
        );
        assert_eq!(
            Manifest::decode(&Manifest::new().encode()),
            Ok(Manifest::new()),
            "empty manifest"
        );
    }

    #[test]
    fn ignores_unknown_keys() {
        let Ok(Value::Dict(mut dict)) = Value::decode(&sample().encode()) else {
            panic!("a manifest encodes as a dictionary");
        };
        dict.insert(b"zz-future".to_vec(), Value::Int(42));
        let decoded = Manifest::decode(&Value::Dict(dict).encode());
        assert_eq!(
            decoded,
            Ok(sample()),
            "unknown keys are a minor format change and must be ignored"
        );
    }

    #[test]
    fn rejects_other_versions() {
        let Ok(Value::Dict(mut dict)) = Value::decode(&sample().encode()) else {
            panic!("a manifest encodes as a dictionary");
        };
        dict.insert(b"v".to_vec(), Value::Int(2));
        assert_eq!(
            Manifest::decode(&Value::Dict(dict).encode()),
            Err(Error::UnsupportedVersion(2))
        );
    }

    #[test]
    fn rejects_dangling_head_and_mixed_formats() {
        let mut manifest = Manifest::new();
        assert!(
            manifest
                .set_head(Some("refs/heads/main".parse().unwrap()))
                .is_err(),
            "HEAD must point at a ref in the manifest"
        );
        manifest
            .insert_ref(
                "refs/heads/a".parse().unwrap(),
                "11".repeat(20).parse().unwrap(),
            )
            .unwrap();
        assert_eq!(
            manifest.insert_ref(
                "refs/heads/b".parse().unwrap(),
                "22".repeat(32).parse().unwrap()
            ),
            Err(Error::MixedObjectFormats)
        );
    }

    #[test]
    fn packs_v2_round_trips_and_must_match_packs() {
        let mut manifest = sample();
        let v2 = vec![InfoHashV2::from([3; 32]), InfoHashV2::from([4; 32])];
        manifest.set_packs_v2(Some(v2.clone())).unwrap();
        let decoded = Manifest::decode(&manifest.encode()).unwrap();
        assert_eq!(
            decoded.packs_v2(),
            Some(v2.as_slice()),
            "packs2 survives a round trip"
        );

        assert!(
            manifest
                .set_packs_v2(Some(vec![InfoHashV2::from([3; 32])]))
                .is_err(),
            "packs2 needs one entry per pack"
        );
        manifest.push_pack(InfoHash::from([5; 20]));
        assert_eq!(
            manifest.packs_v2(),
            None,
            "a new pack invalidates the v2 list"
        );
    }

    #[test]
    fn rejects_bad_mirrors() {
        for mirror in [
            "file:///etc/passwd",
            "ftp://example.org/packs",
            "https://example.org/a b",
            "https://example.org/\n",
        ] {
            assert!(
                Manifest::new()
                    .set_mirrors(vec![mirror.to_owned()])
                    .is_err(),
                "{mirror:?} must be rejected"
            );
        }
        let long = format!("https://example.org/{}", "a".repeat(MAX_MIRROR_LEN));
        assert!(
            Manifest::new().set_mirrors(vec![long]).is_err(),
            "over-long mirrors are rejected"
        );
    }

    #[test]
    fn rejects_malformed_pack_lists() {
        for (key, bytes) in [("packs", vec![0; 32]), ("packs2", vec![0; 20])] {
            let Ok(Value::Dict(mut dict)) = Value::decode(&sample().encode()) else {
                panic!("a manifest encodes as a dictionary");
            };
            dict.insert(key.as_bytes().to_vec(), Value::Bytes(bytes));
            assert!(
                Manifest::decode(&Value::Dict(dict).encode()).is_err(),
                "{key} with the wrong entry width must be rejected"
            );
        }
    }

    #[test]
    fn rejects_hostile_refs() {
        let mut refs = BTreeMap::new();
        refs.insert(
            b"refs/heads/../../hooks/pre-commit".to_vec(),
            Value::Bytes(vec![0; 20]),
        );
        let mut dict = BTreeMap::new();
        dict.insert(b"v".to_vec(), Value::Int(1));
        dict.insert(b"refs".to_vec(), Value::Dict(refs));
        dict.insert(b"packs".to_vec(), Value::Bytes(Vec::new()));
        assert!(
            matches!(
                Manifest::decode(&Value::Dict(dict).encode()),
                Err(Error::InvalidRefName { .. })
            ),
            "path traversal in ref names must be rejected"
        );
    }
}
