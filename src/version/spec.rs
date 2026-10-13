//! nvm's version-spec grammar: what a user may type after `rnm use` or put in
//! a `.nvmrc`.
//!
//! See `docs/research/nvm.md` §2.1. Parsing is purely syntactic; the meaning of
//! aliases, `stable`, `lts/-1`, and so on is decided during resolution.

use std::fmt;
use std::str::FromStr;

use super::exact::{ParseVersionError, Version, parse_number};
use super::range::{NodeRange, ParseRangeError};

/// The prefix of LTS specs and alias names: `lts/*`, `lts/iron`.
pub const LTS_PREFIX: &str = "lts/";

/// The LTS name that means "newest LTS line" (`lts/*`), and the name of the
/// alias file nvm keeps for it.
pub const LATEST_LTS: &str = "*";

/// The prefix of io.js version specs: `iojs-v3.3.1`.
pub const IOJS_PREFIX: &str = "iojs-";

/// A version requirement written with digits: `18`, `v18.1`, `18.1.0`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VersionReq {
  /// `X` or `vX` (a trailing `.` is allowed). Matches any `vX.*.*`.
  Major(u64),
  /// `X.Y` or `vX.Y` (a trailing `.` is allowed). Matches any `vX.Y.*`.
  MajorMinor(u64, u64),
  /// `X.Y.Z[-pre]`. Matches only that version.
  Exact(Version),
}

impl VersionReq {
  /// Returns `true` if `version` satisfies this requirement.
  ///
  /// Partial requirements are anchored at component boundaries, so `1` does
  /// not match `v18.0.0` (nvm's `vX.` prefix match). Prereleases match a
  /// partial requirement; resolution decides whether to prefer releases.
  #[must_use]
  pub fn matches(&self, version: &Version) -> bool {
    match self {
      | Self::Major(major) => version.major == *major,
      | Self::MajorMinor(major, minor) => version.major == *major && version.minor == *minor,
      | Self::Exact(exact) => version == exact,
    }
  }
}

impl FromStr for VersionReq {
  type Err = ParseVersionError;

  fn from_str(input: &str) -> Result<Self, Self::Err> {
    let body = input.strip_prefix('v').unwrap_or(input);
    // A prerelease needs the full X.Y.Z, so hand it to the exact parser.
    if body.contains('-') {
      return input.parse().map(Self::Exact);
    }
    // nvm allows one trailing dot: `18.` and `18.1.` are valid.
    let core = body.strip_suffix('.').unwrap_or(body);
    let parts: Vec<&str> = core.split('.').collect();
    match parts.as_slice() {
      | [major] => Ok(Self::Major(parse_number(major, input)?)),
      | [major, minor] => Ok(Self::MajorMinor(
        parse_number(major, input)?,
        parse_number(minor, input)?,
      )),
      | [_, _, _] => core.parse().map(Self::Exact),
      | _ => Err(ParseVersionError::Malformed(input.to_owned())),
    }
  }
}

impl fmt::Display for VersionReq {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Major(major) => write!(f, "v{major}"),
      | Self::MajorMinor(major, minor) => write!(f, "v{major}.{minor}"),
      | Self::Exact(version) => version.fmt(f),
    }
  }
}

/// Which LTS line an `lts/...` spec refers to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LtsSpec {
  /// `lts/*`: the newest LTS line.
  Latest,
  /// `lts/-N` with N ≥ 1: the Nth LTS line before the newest.
  Offset(u32),
  /// `lts/<codename>`, always lowercase (for example `lts/iron`).
  Named(Box<str>),
}

/// A parsed version spec in nvm's grammar.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VersionSpec {
  /// `18`, `v18.1`, `18.1.0`, `v24.0.0-rc.1`.
  Version(VersionReq),
  /// `node`: the newest installed (or released) Node.js.
  Node,
  /// `stable`: like `node` for every version since 1.0.
  Stable,
  /// `unstable`: the newest odd `0.x` line. Kept for nvm compatibility.
  Unstable,
  /// `iojs` / `io.js` (no version), or `iojs-vX[.Y[.Z]]`.
  Iojs(Option<VersionReq>),
  /// `system`: a Node.js found on `PATH` outside rnm.
  System,
  /// `current`: the active version.
  Current,
  /// `lts/*`, `lts/-N`, `lts/<codename>`.
  Lts(LtsSpec),
  /// Any other name: a user alias such as `default`, followed through
  /// `alias/<name>` files during resolution.
  Alias(Box<str>),
  /// An npm semver range, as `package.json` writes them: `>=18 <21`, `^20.1`,
  /// `20.x`. Not part of nvm's grammar; see [`looks_like_range`].
  Range(NodeRange),
}

/// Why a string is not a valid version spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecError {
  /// Empty input.
  Empty,
  /// Bare `lts`, which nvm rejects with a hint.
  BareLts,
  /// `lts/<Name>` with uppercase letters (nvm exits with code 3).
  LtsUppercase(String),
  /// Malformed `lts/...`, for example `lts/` or `lts/-0`.
  InvalidLts(String),
  /// Looks like a version (starts with a digit, `v<digit>`, or `iojs-`) but
  /// does not parse.
  InvalidVersion(ParseVersionError),
  /// Not a valid alias name.
  InvalidAlias(String),
  /// Written like an npm range, but not a valid one.
  InvalidRange(ParseRangeError),
}

impl fmt::Display for SpecError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Empty => f.write_str("no version given"),
      | Self::BareLts => {
        f.write_str("'lts' is not a version; use 'lts/*' for the newest LTS or 'lts/<codename>'")
      },
      | Self::LtsUppercase(s) => write!(f, "'{s}': LTS names must be lowercase"),
      | Self::InvalidLts(s) => {
        write!(
          f,
          "'{s}' is not a valid LTS spec (expected lts/*, lts/-N, or lts/<codename>)"
        )
      },
      | Self::InvalidVersion(e) => e.fmt(f),
      | Self::InvalidAlias(s) => write!(f, "'{s}' is not a valid version or alias name"),
      | Self::InvalidRange(e) => e.fmt(f),
    }
  }
}

impl std::error::Error for SpecError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      | Self::InvalidVersion(e) => Some(e),
      | Self::InvalidRange(e) => Some(e),
      | _ => None,
    }
  }
}

impl FromStr for VersionSpec {
  type Err = SpecError;

  fn from_str(input: &str) -> Result<Self, Self::Err> {
    match input {
      | "" => return Err(SpecError::Empty),
      | "node" => return Ok(Self::Node),
      | "stable" => return Ok(Self::Stable),
      | "unstable" => return Ok(Self::Unstable),
      | "iojs" | "io.js" => return Ok(Self::Iojs(None)),
      | "system" => return Ok(Self::System),
      | "current" => return Ok(Self::Current),
      | "lts" => return Err(SpecError::BareLts),
      | _ => {},
    }

    if let Some(rest) = input.strip_prefix(LTS_PREFIX) {
      return parse_lts(input, rest).map(Self::Lts);
    }
    if looks_like_range(input) {
      return input
        .parse()
        .map(Self::Range)
        .map_err(SpecError::InvalidRange);
    }
    if let Some(rest) = input.strip_prefix(IOJS_PREFIX) {
      return rest
        .parse()
        .map(|req| Self::Iojs(Some(req)))
        .map_err(SpecError::InvalidVersion);
    }
    if looks_like_version(input) {
      return input
        .parse()
        .map(Self::Version)
        .map_err(SpecError::InvalidVersion);
    }
    if is_valid_alias_name(input) {
      return Ok(Self::Alias(input.into()));
    }
    Err(SpecError::InvalidAlias(input.to_owned()))
  }
}

fn parse_lts(input: &str, rest: &str) -> Result<LtsSpec, SpecError> {
  if rest == LATEST_LTS {
    return Ok(LtsSpec::Latest);
  }
  if let Some(digits) = rest.strip_prefix('-') {
    // nvm's pattern is `lts/-[1-9][0-9]*`: digits only, no leading zero.
    let well_formed = digits.bytes().all(|b| b.is_ascii_digit()) && !digits.starts_with('0');
    return match digits.parse::<u32>() {
      | Ok(n) if well_formed => Ok(LtsSpec::Offset(n)),
      | _ => Err(SpecError::InvalidLts(input.to_owned())),
    };
  }
  if rest.bytes().any(|b| b.is_ascii_uppercase()) {
    return Err(SpecError::LtsUppercase(input.to_owned()));
  }
  if !rest.is_empty() && is_valid_alias_name(rest) {
    return Ok(LtsSpec::Named(rest.into()));
  }
  Err(SpecError::InvalidLts(input.to_owned()))
}

/// Characters that only appear in npm ranges, never in nvm's versions, and
/// (in practice) never in alias names.
const RANGE_MARKERS: &[char] = &['<', '>', '=', '^', '~', '|', '*', ' '];

/// Written as an npm range rather than in nvm's grammar: it has an operator,
/// a space, or an `x` component (`20.x`, `1.2.X`). A bare `x` stays an
/// alias name.
#[must_use]
pub fn looks_like_range(input: &str) -> bool {
  let has_wildcard_component =
    input.contains('.') && input.split('.').any(|part| matches!(part, "x" | "X" | "*"));
  input.trim().contains(RANGE_MARKERS) || has_wildcard_component
}

/// A digit, or `v` followed by a digit. nvm treats these as versions
/// (`nvm_ensure_version_prefix` adds the `v` only before a digit).
fn looks_like_version(input: &str) -> bool {
  let body = input.strip_prefix('v').unwrap_or(input);
  body.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

/// Alias names become file names under `alias/`, so they must be one safe
/// path component. `#` is also excluded because it starts a comment in alias
/// files and `.nvmrc`.
fn is_valid_alias_name(name: &str) -> bool {
  is_valid_alias_component(name) && !name.contains('/')
}

/// One `/`-separated part of an alias name (`lts/iron` has two): non-empty,
/// not `.` or `..`, and without separators, whitespace, control characters,
/// or `#`.
#[must_use]
pub fn is_valid_alias_component(component: &str) -> bool {
  let is_forbidden = |c: char| c == '\\' || c == '#' || c.is_whitespace() || c.is_control();
  let is_relative_marker = component == "." || component == "..";
  !component.is_empty() && !is_relative_marker && !component.contains(is_forbidden)
}

impl fmt::Display for VersionSpec {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Version(req) => req.fmt(f),
      | Self::Node => f.write_str("node"),
      | Self::Stable => f.write_str("stable"),
      | Self::Unstable => f.write_str("unstable"),
      | Self::Iojs(None) => f.write_str("iojs"),
      | Self::Iojs(Some(req)) => write!(f, "{IOJS_PREFIX}{req}"),
      | Self::System => f.write_str("system"),
      | Self::Current => f.write_str("current"),
      | Self::Lts(LtsSpec::Latest) => write!(f, "{LTS_PREFIX}{LATEST_LTS}"),
      | Self::Lts(LtsSpec::Offset(n)) => write!(f, "{LTS_PREFIX}-{n}"),
      | Self::Lts(LtsSpec::Named(name)) => write!(f, "{LTS_PREFIX}{name}"),
      | Self::Alias(name) => f.write_str(name),
      | Self::Range(range) => range.fmt(f),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn spec(s: &str) -> VersionSpec {
    s.parse().unwrap_or_else(|e| panic!("{s:?}: {e}"))
  }

  fn version(s: &str) -> Version {
    s.parse().unwrap()
  }

  #[test]
  fn parses_npm_ranges() {
    for text in [
      ">=18 <21", "^20.1", "~18.2.0", "20.x", "v20.X", "*", "18 || 20", "=20.1.0",
    ] {
      let parsed = spec(text);
      assert!(
        matches!(parsed, VersionSpec::Range(_)),
        "{text}: {parsed:?}"
      );
      assert_eq!(parsed.to_string(), text);
      assert_eq!(spec(&parsed.to_string()), parsed);
    }
    assert!(matches!(
      ">=abc".parse::<VersionSpec>(),
      Err(SpecError::InvalidRange(_))
    ));
    assert_eq!(spec("x"), VersionSpec::Alias("x".into()));
    assert_eq!(
      spec("18.1."),
      VersionSpec::Version(VersionReq::MajorMinor(18, 1))
    );
    assert!(matches!(spec("lts/*"), VersionSpec::Lts(LtsSpec::Latest)));
  }

  #[test]
  fn parses_partial_versions() {
    assert_eq!(spec("18"), VersionSpec::Version(VersionReq::Major(18)));
    assert_eq!(spec("v18"), VersionSpec::Version(VersionReq::Major(18)));
    assert_eq!(spec("18."), VersionSpec::Version(VersionReq::Major(18)));
    assert_eq!(
      spec("v18.1"),
      VersionSpec::Version(VersionReq::MajorMinor(18, 1))
    );
    assert_eq!(
      spec("18.1."),
      VersionSpec::Version(VersionReq::MajorMinor(18, 1))
    );
    assert_eq!(spec("0"), VersionSpec::Version(VersionReq::Major(0)));
  }

  #[test]
  fn parses_exact_versions() {
    assert_eq!(
      spec("18.1.0"),
      VersionSpec::Version(VersionReq::Exact(version("18.1.0")))
    );
    assert_eq!(
      spec("v18.1.0"),
      VersionSpec::Version(VersionReq::Exact(version("18.1.0")))
    );
    assert_eq!(
      spec("18.1.0."),
      VersionSpec::Version(VersionReq::Exact(version("18.1.0")))
    );
    assert_eq!(
      spec("v24.0.0-rc.1"),
      VersionSpec::Version(VersionReq::Exact(version("24.0.0-rc.1")))
    );
  }

  #[test]
  fn rejects_bad_versions() {
    for bad in [
      "18..1",
      "18.1.0.0",
      "18-rc.1",
      "18.1-rc.1",
      "1e3",
      "18..",
      "v1 ",
    ] {
      let result = bad.parse::<VersionSpec>();
      assert!(
        matches!(result, Err(SpecError::InvalidVersion(_))),
        "{bad:?} gave {result:?}"
      );
    }
    // `v` alone does not look like a version, so it is an alias name.
    assert_eq!(spec("v"), VersionSpec::Alias("v".into()));
  }

  #[test]
  fn parses_keywords() {
    assert_eq!(spec("node"), VersionSpec::Node);
    assert_eq!(spec("stable"), VersionSpec::Stable);
    assert_eq!(spec("unstable"), VersionSpec::Unstable);
    assert_eq!(spec("iojs"), VersionSpec::Iojs(None));
    assert_eq!(spec("io.js"), VersionSpec::Iojs(None));
    assert_eq!(spec("system"), VersionSpec::System);
    assert_eq!(spec("current"), VersionSpec::Current);
  }

  #[test]
  fn keywords_are_case_sensitive() {
    // nvm treats `Node` as a user alias name, not as `node`.
    assert_eq!(spec("Node"), VersionSpec::Alias("Node".into()));
  }

  #[test]
  fn parses_iojs_versions() {
    assert_eq!(
      spec("iojs-v3"),
      VersionSpec::Iojs(Some(VersionReq::Major(3)))
    );
    assert_eq!(
      spec("iojs-v3.3.1"),
      VersionSpec::Iojs(Some(VersionReq::Exact(version("3.3.1"))))
    );
    assert!(matches!(
      "iojs-x".parse::<VersionSpec>(),
      Err(SpecError::InvalidVersion(_))
    ));
  }

  #[test]
  fn parses_lts() {
    assert_eq!(spec("lts/*"), VersionSpec::Lts(LtsSpec::Latest));
    assert_eq!(spec("lts/-1"), VersionSpec::Lts(LtsSpec::Offset(1)));
    assert_eq!(spec("lts/-12"), VersionSpec::Lts(LtsSpec::Offset(12)));
    assert_eq!(
      spec("lts/iron"),
      VersionSpec::Lts(LtsSpec::Named("iron".into()))
    );
  }

  #[test]
  fn rejects_bad_lts() {
    assert_eq!("lts".parse::<VersionSpec>(), Err(SpecError::BareLts));
    assert!(matches!(
      "lts/Iron".parse::<VersionSpec>(),
      Err(SpecError::LtsUppercase(_))
    ));
    for bad in [
      "lts/", "lts/-0", "lts/-01", "lts/-", "lts/-+1", "lts/-1a", "lts/a/b", "lts/..",
    ] {
      assert!(
        matches!(bad.parse::<VersionSpec>(), Err(SpecError::InvalidLts(_))),
        "{bad:?} should be InvalidLts"
      );
    }
  }

  #[test]
  fn parses_aliases() {
    assert_eq!(spec("default"), VersionSpec::Alias("default".into()));
    assert_eq!(
      spec("my-project_1.0"),
      VersionSpec::Alias("my-project_1.0".into())
    );
    // `latest` is not an nvm keyword; it is an ordinary alias name.
    assert_eq!(spec("latest"), VersionSpec::Alias("latest".into()));
  }

  #[test]
  fn rejects_bad_aliases() {
    for bad in [".", "..", "a/b", "a\\b", "a#b", "a\tb"] {
      assert!(
        matches!(bad.parse::<VersionSpec>(), Err(SpecError::InvalidAlias(_))),
        "{bad:?} should be InvalidAlias"
      );
    }
    // A space marks an npm range, so this fails as one.
    assert!(matches!(
      "a b".parse::<VersionSpec>(),
      Err(SpecError::InvalidRange(_))
    ));
    assert_eq!("".parse::<VersionSpec>(), Err(SpecError::Empty));
  }

  #[test]
  fn displays_round_trip() {
    for s in [
      "v18",
      "v18.1",
      "v18.1.0",
      "v24.0.0-rc.1",
      "node",
      "stable",
      "unstable",
      "iojs",
      "iojs-v3",
      "system",
      "current",
      "lts/*",
      "lts/-1",
      "lts/iron",
      "default",
    ] {
      assert_eq!(spec(s).to_string(), s);
      assert_eq!(spec(&spec(s).to_string()), spec(s));
    }
  }

  #[test]
  fn partial_requirements_are_anchored() {
    let req = VersionReq::Major(1);
    assert!(req.matches(&version("1.8.4")));
    assert!(!req.matches(&version("18.0.0")));
    assert!(!req.matches(&version("10.1.0")));

    let req = VersionReq::MajorMinor(18, 1);
    assert!(req.matches(&version("18.1.9")));
    assert!(!req.matches(&version("18.10.0")));

    assert!(VersionReq::Major(24).matches(&version("24.0.0-rc.1")));
    assert!(VersionReq::Exact(version("18.1.0")).matches(&version("18.1.0")));
    assert!(!VersionReq::Exact(version("18.1.0")).matches(&version("18.1.1")));
  }
}
