use crate::manifest::MANIFEST_VERSION;

/// Errors produced by `grt-core`.
///
/// Every byte handed to this crate may come from an untrusted peer, so all
/// decoding failures are reported as errors, never panics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Input is not canonical bencode.
    #[error("malformed bencode at byte {offset}: {reason}")]
    Bencode {
        /// Byte offset where decoding failed.
        offset: usize,
        /// What was wrong.
        reason: &'static str,
    },
    /// The manifest declares a format version this build does not understand.
    #[error(
        "unsupported manifest format version {0} (this build reads version {MANIFEST_VERSION})"
    )]
    UnsupportedVersion(i64),
    /// A manifest field is missing or has the wrong shape.
    #[error("manifest field `{field}` is {problem}")]
    InvalidField {
        /// The bencode key of the field.
        field: &'static str,
        /// What was wrong with it.
        problem: &'static str,
    },
    /// A ref name fails git's `check-ref-format` rules.
    #[error("invalid ref name {name:?}: {reason}")]
    InvalidRefName {
        /// The rejected name, lossily decoded for display.
        name: String,
        /// Which rule it broke.
        reason: &'static str,
    },
    /// A fixed-size value has the wrong length.
    #[error("{what} must be {expected} bytes, got {actual}")]
    InvalidLength {
        /// The kind of value.
        what: &'static str,
        /// The accepted length(s).
        expected: &'static str,
        /// The length received.
        actual: usize,
    },
    /// A hex string could not be decoded.
    #[error("invalid hex for {what}")]
    InvalidHex {
        /// The kind of value.
        what: &'static str,
    },
    /// An OpenSSH key is malformed or uses an unsupported algorithm.
    #[error("invalid SSH key: {reason}")]
    InvalidSshKey {
        /// What was wrong with it.
        reason: String,
    },
    /// A trust store line could not be parsed.
    #[error("trust store line {line}: {source}")]
    InvalidTrustStore {
        /// 1-based line number.
        line: usize,
        /// Why the line was rejected.
        source: Box<Error>,
    },
    /// Bytes are not a valid Ed25519 public key.
    #[error("invalid Ed25519 public key")]
    InvalidPublicKey,
    /// A manifest mixes SHA-1 and SHA-256 object ids.
    #[error("manifest mixes SHA-1 and SHA-256 object ids")]
    MixedObjectFormats,
    /// The encoded manifest does not fit in a BEP 44 mutable item.
    #[error("encoded manifest is {size} bytes; the BEP 44 limit is {limit}")]
    TooLarge {
        /// Encoded size in bytes.
        size: usize,
        /// The maximum allowed.
        limit: usize,
    },
    /// The signature does not match the public key, salt, sequence number and value.
    #[error("signature verification failed")]
    BadSignature,
    /// The sequence number cannot be incremented further.
    #[error("sequence number overflow")]
    SeqOverflow,
    /// A remote URL is malformed or unsafe.
    #[error("invalid remote URL {url:?}: {reason}")]
    InvalidUrl {
        /// The rejected URL.
        url: String,
        /// What was wrong with it.
        reason: &'static str,
    },
    /// The operating system's random number generator failed.
    #[error("system random number generator failed")]
    Random,
}
