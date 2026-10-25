use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// A fully qualified git ref name such as `refs/heads/main`.
///
/// Ref names arrive from untrusted peers and end up as paths in the user's
/// repository, so construction enforces git's `check-ref-format` rules and
/// additionally requires the `refs/` prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RefName(String);

impl RefName {
    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RefName {
    type Error = Error;

    fn try_from(name: String) -> Result<Self, Error> {
        match check(&name) {
            Ok(()) => Ok(RefName(name)),
            Err(reason) => Err(Error::InvalidRefName { name, reason }),
        }
    }
}

impl TryFrom<&[u8]> for RefName {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> Result<Self, Error> {
        match std::str::from_utf8(bytes) {
            Ok(name) => RefName::try_from(name.to_owned()),
            Err(_) => Err(Error::InvalidRefName {
                name: String::from_utf8_lossy(bytes).into_owned(),
                reason: "not valid UTF-8",
            }),
        }
    }
}

impl FromStr for RefName {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        RefName::try_from(s.to_owned())
    }
}

impl fmt::Display for RefName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Applies the rules from `git help check-ref-format`.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "git's `.lock` rule is case-sensitive"
)]
fn check(name: &str) -> Result<(), &'static str> {
    let Some(rest) = name.strip_prefix("refs/") else {
        return Err("must start with `refs/`");
    };
    if rest.is_empty() {
        return Err("has no name after `refs/`");
    }
    if name.ends_with('/') || name.ends_with('.') {
        return Err("ends with `/` or `.`");
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return Err("contains `..`, `@{` or `//`");
    }
    if name
        .bytes()
        .any(|b| b.is_ascii_control() || b" ~^:?*[\\".contains(&b))
    {
        return Err("contains a control character, space or one of `~^:?*[\\`");
    }
    for component in rest.split('/') {
        if component.starts_with('.') {
            return Err("has a component starting with `.`");
        }
        if component.ends_with(".lock") {
            return Err("has a component ending with `.lock`");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_refs() {
        for name in [
            "refs/heads/main",
            "refs/tags/v1.0.0",
            "refs/heads/feat/x-y_z",
            "refs/notes/commits",
        ] {
            assert!(name.parse::<RefName>().is_ok(), "{name} is a valid ref");
        }
    }

    #[test]
    fn rejects_unsafe_refs() {
        let cases = [
            "HEAD",
            "heads/main",
            "refs/",
            "refs/heads/../../config",
            "refs/heads/.hidden",
            "refs/heads/main.lock",
            "refs/heads/main/",
            "refs/heads/main.",
            "refs/heads//main",
            "refs/heads/a b",
            "refs/heads/a~1",
            "refs/heads/a^",
            "refs/heads/a:b",
            "refs/heads/a?",
            "refs/heads/a*",
            "refs/heads/a[",
            "refs/heads/a\\b",
            "refs/heads/a@{1}",
            "refs/heads/a\nb",
            "refs/heads/a\u{7f}",
        ];
        for name in cases {
            assert!(
                name.parse::<RefName>().is_err(),
                "{name:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert!(
            RefName::try_from(&b"refs/heads/\xff"[..]).is_err(),
            "ref names must be UTF-8"
        );
    }
}
