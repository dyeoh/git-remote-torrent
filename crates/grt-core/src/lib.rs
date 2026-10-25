//! Core types for git-remote-torrent: signed ref manifests and the newtypes
//! they are built from.
//!
//! [`RemoteUrl`] parses the address git hands the remote helper.
//!
//! A repository's state is a [`Manifest`]: its refs, `HEAD`, the BitTorrent
//! infohashes of the packs holding its objects, and its canonical upstream.
//! A [`PublisherKey`] signs it as a BEP 44 mutable item, producing a
//! [`SignedManifest`] that any peer can check against the publisher's
//! [`PublicKey`].
//!
//! ```
//! use grt_core::{Manifest, PublisherKey, Seq, SignedManifest};
//!
//! let key = PublisherKey::generate()?;
//! let mut manifest = Manifest::new();
//! manifest.insert_ref(
//!     "refs/heads/main".parse()?,
//!     "ce013625030ba8dba906f756967f9e9ca394464a".parse()?,
//! )?;
//! manifest.set_head(Some("refs/heads/main".parse()?))?;
//!
//! let signed = key.sign(&manifest, Seq::new(1), b"")?;
//! let verified = SignedManifest::verify(
//!     key.public_key(),
//!     b"",
//!     Seq::new(1),
//!     signed.value(),
//!     signed.signature(),
//! )?;
//! assert_eq!(verified.manifest(), &manifest);
//! # Ok::<(), grt_core::Error>(())
//! ```
//!
//! This crate is synchronous and performs no I/O.

pub mod bencode;
mod error;
mod info_hash;
mod manifest;
mod object_id;
mod public_key;
mod publisher_key;
mod ref_name;
mod remote_url;
mod signed_manifest;

pub use crate::error::Error;
pub use crate::info_hash::InfoHash;
pub use crate::manifest::{MANIFEST_VERSION, Manifest};
pub use crate::object_id::ObjectId;
pub use crate::public_key::PublicKey;
pub use crate::publisher_key::PublisherKey;
pub use crate::ref_name::RefName;
pub use crate::remote_url::RemoteUrl;
pub use crate::signed_manifest::{MAX_SALT_LEN, MAX_VALUE_LEN, Seq, SignedManifest};
