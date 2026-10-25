use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// A git object id in either of git's object formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum ObjectId {
    /// A SHA-1 object id (git's default `sha1` object format).
    Sha1([u8; 20]),
    /// A SHA-256 object id (`extensions.objectFormat = sha256`).
    Sha256([u8; 32]),
}

impl ObjectId {
    /// Returns the raw id bytes.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            ObjectId::Sha1(bytes) => bytes,
            ObjectId::Sha256(bytes) => bytes,
        }
    }

    /// Returns true if both ids use the same object format.
    pub fn same_format(&self, other: &ObjectId) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl TryFrom<&[u8]> for ObjectId {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        if let Ok(sha1) = <[u8; 20]>::try_from(bytes) {
            Ok(ObjectId::Sha1(sha1))
        } else if let Ok(sha256) = <[u8; 32]>::try_from(bytes) {
            Ok(ObjectId::Sha256(sha256))
        } else {
            Err(Error::InvalidLength {
                what: "object id",
                expected: "20 or 32",
                actual: bytes.len(),
            })
        }
    }
}

impl FromStr for ObjectId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let bytes = hex::decode(s).map_err(|_| Error::InvalidHex { what: "object id" })?;
        ObjectId::try_from(bytes.as_slice())
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_formats() {
        let sha1: ObjectId = "ce013625030ba8dba906f756967f9e9ca394464a".parse().unwrap();
        assert!(
            matches!(sha1, ObjectId::Sha1(_)),
            "40 hex digits is a SHA-1 id"
        );
        assert_eq!(sha1.to_string(), "ce013625030ba8dba906f756967f9e9ca394464a");

        let sha256: ObjectId = "a".repeat(64).parse().unwrap();
        assert!(
            matches!(sha256, ObjectId::Sha256(_)),
            "64 hex digits is a SHA-256 id"
        );
        assert!(
            !sha1.same_format(&sha256),
            "SHA-1 and SHA-256 are different formats"
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!("abc".parse::<ObjectId>().is_err(), "odd-length hex");
        assert!(
            "zz".repeat(20).parse::<ObjectId>().is_err(),
            "non-hex digits"
        );
        assert!(
            "ab".repeat(21).parse::<ObjectId>().is_err(),
            "21 bytes is neither format"
        );
    }
}
