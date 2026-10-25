//! Access to the canonical centralized upstream.
//!
//! [`Upstream`] is the seam between the remote helper and wherever refs and
//! objects come from. [`GitUpstream`] implements it by running the user's own
//! `git` against the real upstream URL, so authentication, proxies and
//! protocol support behave exactly as they would without the helper.

mod error;
mod git_upstream;
mod upstream;

pub use crate::error::Error;
pub use crate::git_upstream::GitUpstream;
pub use crate::upstream::{
    ListedRef, PushResult, PushSpec, PushStatus, RefValue, TransferOptions, Upstream,
};
