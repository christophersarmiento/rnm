//! An exact Node.js version such as `v20.11.1` or `v24.0.0-rc.1`.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// An exact version: `major.minor.patch` plus an optional prerelease tag.
///
/// Parsing accepts an optional leading `v`. Display always adds the `v`, which
/// matches nvm's directory names (`versions/node/v20.11.1`).
///
/// Ordering follows `SemVer` precedence: a prerelease sorts before its release,
/// and prerelease identifiers compare numerically when both are numbers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
  pub major: u64,
  pub minor: u64,
  pub patch: u64,
  /// Prerelease tag without the leading `-`, for example `rc.1` or
  /// `nightly20240101abcdef`.
  pub pre: Option<Box<str>>,
}

/// Why a string is not an exact version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseVersionError {
  /// Not `[v]X.Y.Z[-pre]`.
  Malformed(String),
  /// A component does not fit in a `u64`.
  Overflow(String),
  /// The prerelease tag has characters other than `[0-9A-Za-z.-]`, or empty
  /// dot-separated parts.
  BadPrerelease(String),
}

impl fmt::Display for ParseVersionError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Malformed(s) => write!(f, "'{s}' is not a version (expected X.Y.Z)"),
      | Self::Overflow(s) => write!(f, "'{s}' has a version number that is too large"),
      | Self::BadPrerelease(s) => write!(f, "'{s}' has an invalid prerelease tag"),
    }
  }
}

impl std::error::Error for ParseVersionError {}

impl Version {
  /// Creates a release version with no prerelease tag.
  #[must_use]
  pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
    Self {
      major,
      minor,
      patch,
      pre: None,
    }
  }

  /// Returns `true` for a prerelease (`-rc.1`, `-nightly…`).
  #[must_use]
  pub const fn is_prerelease(&self) -> bool {
    self.pre.is_some()
  }
}

impl FromStr for Version {
  type Err = ParseVersionError;

  fn from_str(input: &str) -> Result<Self, Self::Err> {
    let body = input.strip_prefix('v').unwrap_or(input);
    let (core, pre) = match body.split_once('-') {
      | Some((core, pre)) => (core, Some(pre)),
      | None => (body, None),
    };

    let mut parts = core.split('.');
    let (Some(major), Some(minor), Some(patch), None) =
      (parts.next(), parts.next(), parts.next(), parts.next())
    else {
      return Err(ParseVersionError::Malformed(input.to_owned()));
    };

    let pre = match pre {
      | None => None,
      | Some(pre) if is_valid_prerelease(pre) => Some(pre.into()),
      | Some(_) => return Err(ParseVersionError::BadPrerelease(input.to_owned())),
    };

    Ok(Self {
      major: parse_number(major, input)?,
      minor: parse_number(minor, input)?,
      patch: parse_number(patch, input)?,
      pre,
    })
  }
}

/// Parses one dot-separated numeric component. Only ASCII digits are allowed
/// (no sign, no whitespace).
pub(super) fn parse_number(part: &str, input: &str) -> Result<u64, ParseVersionError> {
  if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
    return Err(ParseVersionError::Malformed(input.to_owned()));
  }
  part
    .parse()
    .map_err(|_| ParseVersionError::Overflow(input.to_owned()))
}

/// nvm's prerelease rule (`nvm_is_valid_version`): non-empty, only
/// `[0-9A-Za-z.-]`, and no empty dot-separated parts.
fn is_valid_prerelease(pre: &str) -> bool {
  !pre.is_empty()
    && pre
      .bytes()
      .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    && pre.split('.').all(|part| !part.is_empty())
}

impl fmt::Display for Version {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)?;
    if let Some(pre) = &self.pre {
      write!(f, "-{pre}")?;
    }
    Ok(())
  }
}

impl Ord for Version {
  fn cmp(&self, other: &Self) -> Ordering {
    (self.major, self.minor, self.patch)
      .cmp(&(other.major, other.minor, other.patch))
      .then_with(|| match (&self.pre, &other.pre) {
        | (None, None) => Ordering::Equal,
        | (None, Some(_)) => Ordering::Greater,
        | (Some(_), None) => Ordering::Less,
        | (Some(a), Some(b)) => compare_prerelease(a, b),
      })
  }
}

impl PartialOrd for Version {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

/// `SemVer` 2.0 prerelease precedence: compare dot-separated identifiers left to
/// right; numeric identifiers compare as numbers and sort before alphanumeric
/// ones; a shorter list sorts first when all shared identifiers are equal.
fn compare_prerelease(a: &str, b: &str) -> Ordering {
  let mut left = a.split('.');
  let mut right = b.split('.');
  loop {
    match (left.next(), right.next()) {
      | (None, None) => return Ordering::Equal,
      | (None, Some(_)) => return Ordering::Less,
      | (Some(_), None) => return Ordering::Greater,
      | (Some(x), Some(y)) => {
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
          | (Ok(x), Ok(y)) => x.cmp(&y),
          | (Ok(_), Err(_)) => Ordering::Less,
          | (Err(_), Ok(_)) => Ordering::Greater,
          | (Err(_), Err(_)) => x.cmp(y),
        };
        if order != Ordering::Equal {
          return order;
        }
      },
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn v(s: &str) -> Version {
    s.parse().unwrap()
  }

  #[test]
  fn parses_with_and_without_prefix() {
    assert_eq!(v("v20.11.1"), Version::new(20, 11, 1));
    assert_eq!(v("20.11.1"), Version::new(20, 11, 1));
    assert_eq!(v("0.10.48"), Version::new(0, 10, 48));
  }

  #[test]
  fn parses_prerelease() {
    let rc = v("v24.0.0-rc.1");
    assert_eq!(rc.pre.as_deref(), Some("rc.1"));
    assert!(rc.is_prerelease());
    assert_eq!(rc.to_string(), "v24.0.0-rc.1");
    assert_eq!(
      v("v23.0.0-nightly20240101abcdef").pre.as_deref(),
      Some("nightly20240101abcdef")
    );
    assert_eq!(
      v("v8.0.0-v8-canary2017").pre.as_deref(),
      Some("v8-canary2017")
    );
  }

  #[test]
  fn rejects_malformed() {
    for bad in [
      "",
      "v",
      "20",
      "20.1",
      "20.1.1.1",
      "v20..1",
      "20.1.",
      "x.1.1",
      "+1.2.3",
      " 1.2.3",
      "1.2.3 ",
      "vv1.2.3",
      "1.2.3-",
      "1.2.3-rc..1",
      "1.2.3-rc.",
      "1.2.3-.rc",
      "1.2.3-rc_1",
      "1.2.3-rc/1",
    ] {
      assert!(bad.parse::<Version>().is_err(), "{bad:?} should not parse");
    }
  }

  #[test]
  fn rejects_overflow() {
    assert!(matches!(
      "99999999999999999999.0.0".parse::<Version>(),
      Err(ParseVersionError::Overflow(_))
    ));
  }

  #[test]
  fn displays_with_prefix() {
    assert_eq!(v("18.0.0").to_string(), "v18.0.0");
  }

  #[test]
  fn orders_numerically() {
    assert!(v("v9.0.0") < v("v10.0.0"));
    assert!(v("v18.9.0") < v("v18.10.0"));
    assert!(v("v18.10.2") < v("v18.10.10"));
  }

  #[test]
  fn orders_prereleases_before_release() {
    assert!(v("v24.0.0-rc.1") < v("v24.0.0"));
    assert!(v("v24.0.0-rc.2") < v("v24.0.0-rc.10"));
    assert!(v("v24.0.0-rc.1") < v("v24.0.0-rc.1.1"));
    assert!(v("v24.0.0-1") < v("v24.0.0-rc"));
    assert!(v("v24.0.0-alpha") < v("v24.0.0-beta"));
    assert!(v("v23.9.9") < v("v24.0.0-rc.1"));
  }
}
