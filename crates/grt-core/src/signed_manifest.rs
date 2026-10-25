use std::collections::BTreeMap;

use crate::bencode::Value;
use crate::error::Error;
use crate::manifest::Manifest;
use crate::public_key::PublicKey;

/// Largest bencoded `v` a BEP 44 mutable item may carry, in bytes.
///
/// The manifest travels as a bencode byte string, so its own length prefix
/// (`NNN:`) counts toward this limit.
pub const MAX_VALUE_LEN: usize = 1000;

/// Largest BEP 44 salt, in bytes.
pub const MAX_SALT_LEN: usize = 64;

/// A BEP 44 sequence number. Each new manifest for the same key and salt must
/// use a higher one; peers keep the highest they have verified, which stops
/// rollback to older refs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Seq(i64);

impl Seq {
    /// Wraps a raw sequence number.
    pub fn new(seq: i64) -> Self {
        Seq(seq)
    }

    /// Returns the raw sequence number.
    pub fn get(self) -> i64 {
        self.0
    }

    /// Returns the following sequence number.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SeqOverflow`] at `i64::MAX`.
    pub fn next(self) -> Result<Seq, Error> {
        self.0.checked_add(1).map(Seq).ok_or(Error::SeqOverflow)
    }
}

/// A manifest together with the BEP 44 signature that authenticates it.
///
/// Only constructible by signing or by verifying, so holding one means the
/// signature has been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedManifest {
    public_key: PublicKey,
    salt: Vec<u8>,
    seq: Seq,
    value: Vec<u8>,
    signature: [u8; 64],
    manifest: Manifest,
}

impl SignedManifest {
    pub(crate) fn create(
        manifest: &Manifest,
        seq: Seq,
        salt: &[u8],
        public_key: PublicKey,
        sign: impl FnOnce(&[u8]) -> [u8; 64],
    ) -> Result<Self, Error> {
        check_salt(salt)?;
        let value = manifest.encode();
        check_value_len(&value)?;
        let signature = sign(&signable(salt, seq, &value));
        Ok(SignedManifest {
            public_key,
            salt: salt.to_vec(),
            seq,
            value,
            signature,
            manifest: manifest.clone(),
        })
    }

    /// Verifies a BEP 44 mutable item, as returned by a DHT `get`, and decodes
    /// its manifest.
    ///
    /// The signature is checked before the value is parsed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BadSignature`] if the signature does not cover exactly
    /// this key, salt, sequence number and value; [`Error::TooLarge`] or
    /// [`Error::InvalidLength`] if the value or salt exceed BEP 44 limits; or
    /// any [`Manifest::decode`] error.
    pub fn verify(
        public_key: PublicKey,
        salt: &[u8],
        seq: Seq,
        value: &[u8],
        signature: &[u8; 64],
    ) -> Result<Self, Error> {
        check_salt(salt)?;
        check_value_len(value)?;
        public_key.verify(&signable(salt, seq, value), signature)?;
        Ok(SignedManifest {
            public_key,
            salt: salt.to_vec(),
            seq,
            value: value.to_vec(),
            signature: *signature,
            manifest: Manifest::decode(value)?,
        })
    }

    /// The publisher's public key.
    pub fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// The BEP 44 salt (empty if unused).
    pub fn salt(&self) -> &[u8] {
        &self.salt
    }

    /// The sequence number.
    pub fn seq(&self) -> Seq {
        self.seq
    }

    /// The exact signed bytes: the bencoded manifest, as stored in the DHT.
    pub fn value(&self) -> &[u8] {
        &self.value
    }

    /// The 64-byte Ed25519 signature.
    pub fn signature(&self) -> &[u8; 64] {
        &self.signature
    }

    /// The decoded manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Encodes the signed item for local caching (keys `k`, `salt`, `seq`, `sig`, `v`).
    pub fn encode(&self) -> Vec<u8> {
        let mut dict = BTreeMap::new();
        dict.insert(
            b"k".to_vec(),
            Value::Bytes(self.public_key.to_bytes().to_vec()),
        );
        dict.insert(b"salt".to_vec(), Value::Bytes(self.salt.clone()));
        dict.insert(b"seq".to_vec(), Value::Int(self.seq.get()));
        dict.insert(b"sig".to_vec(), Value::Bytes(self.signature.to_vec()));
        dict.insert(b"v".to_vec(), Value::Bytes(self.value.clone()));
        Value::Dict(dict).encode()
    }

    /// Decodes a cached item produced by [`SignedManifest::encode`] and
    /// re-verifies its signature.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Bencode`] or [`Error::InvalidField`] if the item is
    /// malformed, and any [`SignedManifest::verify`] error.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let value = Value::decode(bytes)?;
        let dict = value.as_dict().ok_or(Error::InvalidField {
            field: "signed manifest",
            problem: "not a dictionary",
        })?;
        let field = |key: &'static str| {
            dict.get(key.as_bytes()).ok_or(Error::InvalidField {
                field: key,
                problem: "missing",
            })
        };
        let bytes_field = |key: &'static str| {
            field(key)?.as_bytes().ok_or(Error::InvalidField {
                field: key,
                problem: "not a byte string",
            })
        };

        let public_key = PublicKey::try_from(bytes_field("k")?)?;
        let seq = field("seq")?.as_int().map(Seq).ok_or(Error::InvalidField {
            field: "seq",
            problem: "not an integer",
        })?;
        let signature = bytes_field("sig")?;
        let signature = <&[u8; 64]>::try_from(signature).map_err(|_| Error::InvalidLength {
            what: "signature",
            expected: "64",
            actual: signature.len(),
        })?;
        SignedManifest::verify(
            public_key,
            bytes_field("salt")?,
            seq,
            bytes_field("v")?,
            signature,
        )
    }
}

/// Builds the exact byte string BEP 44 signs: the bencoded `salt` (if any),
/// `seq` and `v` entries, without the enclosing dictionary.
///
/// `v` is the manifest carried as a bencode *byte string*
/// (`1:v<len>:<manifest>`), which is how DHT implementations such as
/// `mainline` store and verify values. See ADR-0006.
fn signable(salt: &[u8], seq: Seq, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + salt.len() + 32);
    if !salt.is_empty() {
        out.extend_from_slice(b"4:salt");
        out.extend_from_slice(&Value::Bytes(salt.to_vec()).encode());
    }
    out.extend_from_slice(b"3:seq");
    out.extend_from_slice(&Value::Int(seq.get()).encode());
    out.extend_from_slice(b"1:v");
    out.extend_from_slice(format!("{}:", value.len()).as_bytes());
    out.extend_from_slice(value);
    out
}

fn check_salt(salt: &[u8]) -> Result<(), Error> {
    if salt.len() > MAX_SALT_LEN {
        return Err(Error::InvalidLength {
            what: "salt",
            expected: "at most 64",
            actual: salt.len(),
        });
    }
    Ok(())
}

fn check_value_len(value: &[u8]) -> Result<(), Error> {
    let encoded = value.len() + value.len().to_string().len() + 1;
    if encoded > MAX_VALUE_LEN {
        return Err(Error::TooLarge {
            size: encoded,
            limit: MAX_VALUE_LEN,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher_key::PublisherKey;

    fn manifest() -> Manifest {
        let mut manifest = Manifest::new();
        manifest
            .insert_ref(
                "refs/heads/main".parse().unwrap(),
                "ce013625030ba8dba906f756967f9e9ca394464a".parse().unwrap(),
            )
            .unwrap();
        manifest
    }

    #[test]
    fn signable_matches_bep44_layout() {
        // Byte layouts from the BEP 44 test vectors: v = "Hello World!", seq 1.
        assert_eq!(
            signable(b"", Seq(1), b"Hello World!"),
            b"3:seqi1e1:v12:Hello World!"
        );
        assert_eq!(
            signable(b"foobar", Seq(1), b"Hello World!"),
            b"4:salt6:foobar3:seqi1e1:v12:Hello World!"
        );
    }

    #[test]
    fn sign_then_verify() {
        let key = PublisherKey::generate().unwrap();
        let signed = key.sign(&manifest(), Seq(7), b"repo").unwrap();
        let verified = SignedManifest::verify(
            key.public_key(),
            b"repo",
            Seq(7),
            signed.value(),
            signed.signature(),
        )
        .unwrap();
        assert_eq!(
            verified, signed,
            "verification must reproduce the signed item"
        );
        assert_eq!(verified.manifest(), &manifest());
    }

    #[test]
    fn rejects_any_tampering() {
        let key = PublisherKey::generate().unwrap();
        let signed = key.sign(&manifest(), Seq(7), b"repo").unwrap();
        let (value, signature) = (signed.value(), signed.signature());

        let mut other = manifest();
        other.set_upstream(Some("https://evil.example/repo.git".to_owned()));
        let other_value = other.encode();
        let mut flipped = *signature;
        flipped[0] ^= 1;
        let other_key = PublisherKey::generate().unwrap().public_key();

        let cases: [(&str, Result<SignedManifest, Error>); 5] = [
            (
                "rollback to another seq",
                SignedManifest::verify(key.public_key(), b"repo", Seq(6), value, signature),
            ),
            (
                "different salt",
                SignedManifest::verify(key.public_key(), b"other", Seq(7), value, signature),
            ),
            (
                "different value",
                SignedManifest::verify(key.public_key(), b"repo", Seq(7), &other_value, signature),
            ),
            (
                "flipped signature bit",
                SignedManifest::verify(key.public_key(), b"repo", Seq(7), value, &flipped),
            ),
            (
                "different key",
                SignedManifest::verify(other_key, b"repo", Seq(7), value, signature),
            ),
        ];
        for (what, result) in cases {
            assert_eq!(
                result,
                Err(Error::BadSignature),
                "{what} must fail verification"
            );
        }
    }

    #[test]
    fn cache_encoding_round_trips_and_reverifies() {
        let key = PublisherKey::generate().unwrap();
        let signed = key.sign(&manifest(), Seq(3), b"").unwrap();
        let encoded = signed.encode();
        assert_eq!(
            SignedManifest::decode(&encoded),
            Ok(signed),
            "cache round trip"
        );

        let Ok(Value::Dict(mut dict)) = Value::decode(&encoded) else {
            panic!("cache format is a dictionary");
        };
        dict.insert(b"seq".to_vec(), Value::Int(4));
        assert_eq!(
            SignedManifest::decode(&Value::Dict(dict).encode()),
            Err(Error::BadSignature),
            "editing the cached seq must invalidate it"
        );
    }

    #[test]
    fn enforces_bep44_limits() {
        let key = PublisherKey::generate().unwrap();
        assert!(
            matches!(
                key.sign(&manifest(), Seq(1), &[0; 65]),
                Err(Error::InvalidLength { what: "salt", .. })
            ),
            "salts over 64 bytes are rejected"
        );

        let mut big = Manifest::new();
        for i in 0..40 {
            big.insert_ref(format!("refs/heads/branch-{i}").parse().unwrap(), zero_id())
                .unwrap();
        }
        assert!(
            matches!(key.sign(&big, Seq(1), b""), Err(Error::TooLarge { .. })),
            "manifests over 1000 bytes do not fit in a BEP 44 item"
        );
    }

    fn zero_id() -> crate::ObjectId {
        "00".repeat(20).parse().unwrap()
    }
}
