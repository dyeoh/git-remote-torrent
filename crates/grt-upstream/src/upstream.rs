use std::fmt;
use std::str::FromStr;

use grt_core::ObjectId;

use crate::error::Error;

/// Where refs and objects come from and go to.
pub trait Upstream {
    /// Lists the upstream's refs, including `HEAD`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream cannot be reached or answers nonsense.
    fn list(&self, options: &TransferOptions) -> Result<Vec<ListedRef>, Error>;

    /// Fetches the objects reachable from `refs` into the local repository
    /// without updating any local refs.
    ///
    /// # Errors
    ///
    /// Returns an error if the fetch fails.
    fn fetch(&self, refs: &[String], options: &TransferOptions) -> Result<(), Error>;

    /// Pushes `specs` and reports a result for each one, in order.
    ///
    /// # Errors
    ///
    /// Returns an error only if the push failed as a whole; per-ref rejections
    /// are reported as [`PushStatus::Rejected`].
    fn push(&self, specs: &[PushSpec], options: &TransferOptions)
    -> Result<Vec<PushResult>, Error>;
}

/// Settings git passes to the helper with `option`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TransferOptions {
    /// 0 is quiet, 1 is normal, 2+ is verbose.
    pub verbosity: u32,
    /// Whether to show progress; `None` lets git decide.
    pub progress: Option<bool>,
}

impl Default for TransferOptions {
    fn default() -> Self {
        TransferOptions {
            verbosity: 1,
            progress: None,
        }
    }
}

/// A ref as advertised by the upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedRef {
    /// The full ref name, or `HEAD`.
    pub name: String,
    /// What it points at.
    pub value: RefValue,
}

/// The target of a [`ListedRef`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefValue {
    /// A direct ref to an object.
    Object(ObjectId),
    /// A symbolic ref to another ref, as `HEAD` usually is.
    Symbolic(String),
}

impl ListedRef {
    /// A ref pointing directly at an object.
    pub fn object(name: impl Into<String>, id: ObjectId) -> Self {
        ListedRef {
            name: name.into(),
            value: RefValue::Object(id),
        }
    }

    /// A symbolic ref pointing at another ref.
    pub fn symbolic(name: impl Into<String>, target: impl Into<String>) -> Self {
        ListedRef {
            name: name.into(),
            value: RefValue::Symbolic(target.into()),
        }
    }
}

/// One `[+]<src>:<dst>` push refspec. An empty `src` deletes `dst`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSpec {
    /// Whether this is a forced update (`+`).
    pub force: bool,
    /// The local source ref; empty to delete.
    pub src: String,
    /// The remote ref to update.
    pub dst: String,
}

impl FromStr for PushSpec {
    type Err = Error;

    fn from_str(spec: &str) -> Result<Self, Error> {
        let (force, rest) = match spec.strip_prefix('+') {
            Some(rest) => (true, rest),
            None => (false, spec),
        };
        let invalid = || Error::InvalidPushSpec(spec.to_owned());
        let (src, dst) = rest.split_once(':').ok_or_else(invalid)?;
        // Refs become arguments to `git push`; none may look like an option.
        if dst.is_empty() || src.starts_with('-') || dst.starts_with('-') {
            return Err(invalid());
        }
        Ok(PushSpec {
            force,
            src: src.to_owned(),
            dst: dst.to_owned(),
        })
    }
}

impl fmt::Display for PushSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plus = if self.force { "+" } else { "" };
        write!(f, "{plus}{}:{}", self.src, self.dst)
    }
}

/// The outcome of pushing one ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushResult {
    /// The remote ref.
    pub dst: String,
    /// Whether it was updated.
    pub status: PushStatus,
}

/// Whether a ref update was accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStatus {
    /// Updated, or already up to date.
    Ok,
    /// Refused, with git's reason (for example `non-fast-forward`).
    Rejected(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_spec_round_trip() {
        for spec in [
            "refs/heads/main:refs/heads/main",
            "+refs/heads/a:refs/heads/b",
            ":refs/heads/gone",
        ] {
            assert_eq!(
                spec.parse::<PushSpec>().unwrap().to_string(),
                spec,
                "{spec} must round-trip"
            );
        }
    }

    #[test]
    fn push_spec_rejects_options_and_garbage() {
        for spec in [
            "refs/heads/main",
            "--mirror:refs/heads/x",
            "refs/heads/a:--force",
            "refs/heads/a:",
        ] {
            assert!(
                spec.parse::<PushSpec>().is_err(),
                "{spec:?} must be rejected"
            );
        }
    }
}
