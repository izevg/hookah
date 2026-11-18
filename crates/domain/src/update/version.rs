//! Release versions: strict `MAJOR.MINOR.PATCH`, ordered numerically.

use std::fmt;

/// Why a version string was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionError {
    /// Not three dot-separated whole numbers.
    Malformed,
    /// Has a prerelease or build suffix (`1.0.0-rc.1`, `1.0.0+7`); never an update candidate.
    Prerelease,
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("not a version of the form X.Y.Z"),
            Self::Prerelease => f.write_str("a prerelease"),
        }
    }
}

/// A stable release version. The derived order is the semantic-version order, because the fields
/// are compared in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// `0.0.0`: the version of a build that is not a release.
    pub const DEV: Self = Self {
        major: 0,
        minor: 0,
        patch: 0,
    };

    /// Parses `X.Y.Z`, with an optional leading `v` (a tag name).
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let core = match text.find(['-', '+']) {
            Some(at) => {
                Self::parse_core(&text[..at])?;
                return Err(VersionError::Prerelease);
            }
            None => text,
        };
        Self::parse_core(core)
    }

    fn parse_core(core: &str) -> Result<Self, VersionError> {
        let mut parts = core.split('.');
        let mut next = || {
            let part = parts.next().ok_or(VersionError::Malformed)?;
            // No leading zeros; `parse` itself refuses an empty part, a sign, and anything but digits.
            if part.len() > 1 && part.starts_with('0') {
                return Err(VersionError::Malformed);
            }
            part.parse::<u64>().map_err(|_| VersionError::Malformed)
        };
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        if parts.next().is_some() {
            return Err(VersionError::Malformed);
        }
        Ok(version)
    }

    /// True for `0.0.0`: what a build without a release version reports.
    pub fn is_dev(&self) -> bool {
        *self == Self::DEV
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn parses_plain_and_tag_forms() {
        assert_eq!(v("1.2.3").to_string(), "1.2.3");
        assert_eq!(v("v10.20.30").to_string(), "10.20.30");
        assert_eq!(v(" 0.4.0\n").to_string(), "0.4.0");
    }

    #[test]
    fn refuses_what_is_not_a_plain_version() {
        for bad in [
            "", "1", "1.2", "1.2.3.4", "a.b.c", "1..3", "1.2.x", "01.2.3", "1.2.-3", "v", "1.2.3 4",
        ] {
            assert_eq!(Version::parse(bad), Err(VersionError::Malformed), "{bad:?}");
        }
        assert_eq!(
            Version::parse("99999999999999999999.0.0"),
            Err(VersionError::Malformed)
        );
    }

    #[test]
    fn a_prerelease_or_build_suffix_is_refused_as_such() {
        for pre in ["1.0.0-rc.1", "v1.0.0-beta", "1.0.0+5", "2.0.0-alpha+1"] {
            assert_eq!(Version::parse(pre), Err(VersionError::Prerelease), "{pre}");
        }
        // A suffix on garbage is still garbage.
        assert_eq!(Version::parse("x-1"), Err(VersionError::Malformed));
    }

    #[test]
    fn orders_numerically_not_lexically() {
        assert!(v("1.10.0") > v("1.9.9"));
        assert!(v("2.0.0") > v("1.99.99"));
        assert!(v("0.4.1") > v("0.4.0"));
        assert!(v("1.0.10") > v("1.0.9"));
        assert!(v("1.2.3") == v("v1.2.3"));
        assert!(v("0.0.1") > v("0.0.0"));
    }

    #[test]
    fn only_zero_zero_zero_is_a_dev_build() {
        assert!(v("0.0.0").is_dev());
        assert!(!v("0.0.1").is_dev());
        assert!(!v("0.1.0").is_dev());
        assert!(!v("1.0.0").is_dev());
    }

    #[test]
    fn errors_describe_themselves() {
        assert!(VersionError::Malformed.to_string().contains("X.Y.Z"));
        assert!(VersionError::Prerelease.to_string().contains("prerelease"));
    }
}
