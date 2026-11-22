use std::{cmp::Ordering, fmt, str::FromStr};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid version: {0}")]
pub struct VersionError(String);

/// `MAJOR.MINOR.PATCH` with an optional `-prerelease` suffix. A prerelease
/// sorts before the release it leads up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
    pre: Option<String>,
}

impl FromStr for Version {
    type Err = VersionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || VersionError(value.to_string());
        let text = value.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };

        let mut parts = core.split('.').map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            part.parse::<u32>().ok()
        });
        let mut next = || parts.next().flatten().ok_or_else(invalid);
        let (major, minor, patch) = (next()?, next()?, next()?);

        if parts.next().is_some() {
            return Err(invalid());
        }
        if let Some(pre) = pre {
            let valid = !pre.is_empty()
                && pre
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.');
            if !valid {
                return Err(invalid());
            }
        }

        Ok(Self {
            major,
            minor,
            patch,
            pre: pre.map(str::to_string),
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => left.cmp(right),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        match &self.pre {
            Some(pre) => write!(f, "-{pre}"),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        text.parse().unwrap()
    }

    #[test]
    fn orders_numerically_not_lexically() {
        assert!(version("0.10.0") > version("0.9.9"));
        assert!(version("1.0.0") > version("0.99.99"));
        assert_eq!(version("v1.2.3"), version("1.2.3"));
    }

    #[test]
    fn prerelease_sorts_before_release() {
        assert!(version("1.1.0-rc1") < version("1.1.0"));
        assert!(version("1.1.0-rc1") > version("1.0.9"));
        assert!(version("1.1.0-rc1") < version("1.1.0-rc2"));
    }

    #[test]
    fn rejects_malformed_versions() {
        for text in [
            "", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.3-", "+1.2.3", "1..3",
        ] {
            assert!(
                text.parse::<Version>().is_err(),
                "{text:?} must be rejected"
            );
        }
    }
}
