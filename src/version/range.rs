//! npm's semver range grammar, as used by `engines.node` and
//! `devEngines.runtime.version` in `package.json`.
//!
//! See <https://github.com/npm/node-semver#ranges>. Ranges are desugared the
//! same way node-semver does it: carets, tildes, x-ranges, and hyphen ranges
//! become plain `<`, `<=`, `>`, `>=`, and `=` comparators.

use std::fmt;
use std::str::FromStr;

use super::exact::Version;

/// Separates alternative comparator sets: `>=16 || ^14.17`.
const OR_SEPARATOR: &str = "||";

/// The middle word of a hyphen range: `1.2.3 - 2.3.4`.
const HYPHEN: &str = "-";

/// Characters that make up comparison operators. A word made only of these
/// is an operator separated from its version by whitespace (`>= 18`).
const OPERATOR_CHARS: &[char] = &['<', '>', '=', '~', '^'];

/// Characters node-semver ignores in front of a version: `v1.2.3`, `=1.2.3`.
const VERSION_PREFIX_CHARS: &[char] = &['v', '='];

/// Components that mean "any number": `1.x`, `1.X`, `1.*`.
const WILDCARDS: &[&str] = &["x", "X", "*"];

/// The lowest possible prerelease tag. An upper bound of `<2.0.0-0` excludes
/// every `2.0.0` prerelease as well as `2.0.0` itself.
const LOWEST_PRERELEASE: &str = "0";

/// A parsed npm semver range such as `>=18 <21`, `^20.1`, or
/// `>=16 || ^14.17`.
///
/// The grammar and the matching rules follow node-semver's `Range` with
/// default options; see <https://github.com/npm/node-semver#ranges>.
/// Parsing is whitespace-tolerant (`>= 18`, `~> 1`), and the empty string,
/// `*`, `x`, and `X` mean "any version".
///
/// Display prints the trimmed source text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeRange {
  source: Box<str>,
  /// The range matches if any of these sets matches.
  alternatives: Vec<ComparatorSet>,
}

/// Comparators that must all match. An empty set matches every release.
type ComparatorSet = Vec<Comparator>;

/// Why a string is not a semver range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseRangeError {
  range: String,
  comparator: String,
}

impl fmt::Display for ParseRangeError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "'{}' is not a valid semver range ('{}' is not a valid comparator)",
      self.range, self.comparator
    )
  }
}

impl std::error::Error for ParseRangeError {}

impl NodeRange {
  /// Returns `true` if `version` satisfies the range, like node-semver's
  /// `satisfies` with default options.
  ///
  /// A prerelease version matches a comparator set only if some comparator in
  /// that set names a prerelease of the same `major.minor.patch`. So
  /// `>=20.0.0-rc.1` matches `v20.0.0-rc.2`, but `*` and `>=19` match no
  /// prerelease at all.
  #[must_use]
  pub fn matches(&self, version: &Version) -> bool {
    self
      .alternatives
      .iter()
      .any(|set| set_matches(set, version))
  }

  /// Returns the highest version in `versions` that satisfies the range, or
  /// `None` if no version does.
  #[must_use]
  pub fn max_satisfying<'a>(
    &self,
    versions: impl IntoIterator<Item = &'a Version>,
  ) -> Option<&'a Version> {
    versions
      .into_iter()
      .filter(|version| self.matches(version))
      .max()
  }
}

fn set_matches(set: &[Comparator], version: &Version) -> bool {
  if !set.iter().all(|comparator| comparator.matches(version)) {
    return false;
  }
  if !version.is_prerelease() {
    return true;
  }
  set
    .iter()
    .any(|comparator| comparator.allows_prereleases_of(version))
}

impl FromStr for NodeRange {
  type Err = ParseRangeError;

  fn from_str(input: &str) -> Result<Self, Self::Err> {
    let source = input.trim();
    let alternatives = source
      .split(OR_SEPARATOR)
      .map(parse_set)
      .collect::<Result<_, _>>()
      .map_err(|comparator| ParseRangeError {
        range: source.to_owned(),
        comparator,
      })?;
    Ok(Self {
      source: source.into(),
      alternatives,
    })
  }
}

impl fmt::Display for NodeRange {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.source)
  }
}

/// A primitive comparison against one exact version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Op {
  Less,
  LessOrEqual,
  Greater,
  GreaterOrEqual,
  Equal,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Comparator {
  op: Op,
  version: Version,
}

impl Comparator {
  fn matches(&self, version: &Version) -> bool {
    let order = version.cmp(&self.version);
    match self.op {
      | Op::Less => order.is_lt(),
      | Op::LessOrEqual => order.is_le(),
      | Op::Greater => order.is_gt(),
      | Op::GreaterOrEqual => order.is_ge(),
      | Op::Equal => order.is_eq(),
    }
  }

  fn allows_prereleases_of(&self, version: &Version) -> bool {
    let bound = &self.version;
    bound.is_prerelease()
      && (bound.major, bound.minor, bound.patch) == (version.major, version.minor, version.patch)
  }
}

/// The operator in front of a version, before desugaring.
#[derive(Clone, Copy, Debug)]
enum Operator {
  Caret,
  Tilde,
  Primitive(Op),
}

/// Longer spellings come first so that `>=` is not read as `>`.
const OPERATORS: [(&str, Operator); 8] = [
  ("~>", Operator::Tilde),
  ("~", Operator::Tilde),
  ("^", Operator::Caret),
  (">=", Operator::Primitive(Op::GreaterOrEqual)),
  ("<=", Operator::Primitive(Op::LessOrEqual)),
  (">", Operator::Primitive(Op::Greater)),
  ("<", Operator::Primitive(Op::Less)),
  ("=", Operator::Primitive(Op::Equal)),
];

/// A version where trailing components may be missing or wildcards.
/// Everything after the first wildcard counts as a wildcard (`1.x.3` is `1.x`).
#[derive(Clone, Debug)]
enum Partial {
  Any,
  Major(u64),
  MajorMinor(u64, u64),
  Exact(Version),
}

/// Marks a comparator that does not parse or desugar.
struct Invalid;

/// Parses one `||`-separated set. On failure, returns the offending text.
fn parse_set(text: &str) -> Result<ComparatorSet, String> {
  let words: Vec<&str> = text.split_whitespace().collect();
  if let [from, HYPHEN, to] = words.as_slice() {
    return parse_partial(from)
      .and_then(|from| hyphen(from, parse_partial(to)?))
      .map_err(|Invalid| text.trim().to_owned());
  }

  let mut set = ComparatorSet::new();
  let mut words = words.into_iter();
  while let Some(word) = words.next() {
    let is_lone_operator = word.chars().all(|c| OPERATOR_CHARS.contains(&c));
    let comparator = if is_lone_operator {
      let operand = words.next().ok_or_else(|| word.to_owned())?;
      format!("{word}{operand}")
    } else {
      word.to_owned()
    };
    match parse_comparator(&comparator) {
      | Ok(desugared) => set.extend(desugared),
      | Err(Invalid) => return Err(comparator),
    }
  }
  Ok(set)
}

fn parse_comparator(text: &str) -> Result<ComparatorSet, Invalid> {
  let (operator, operand) = OPERATORS
    .iter()
    .find_map(|&(spelling, operator)| Some((operator, text.strip_prefix(spelling)?)))
    .unwrap_or((Operator::Primitive(Op::Equal), text));
  let partial = parse_partial(operand)?;
  match operator {
    | Operator::Caret => caret(partial),
    | Operator::Tilde => tilde(partial),
    | Operator::Primitive(op) => primitive(op, partial),
  }
}

fn parse_partial(text: &str) -> Result<Partial, Invalid> {
  let text = text.trim_start_matches(VERSION_PREFIX_CHARS);
  let text = match text.split_once('+') {
    | Some((text, build)) if is_valid_identifier_list(build) => text,
    | Some(_) => return Err(Invalid),
    | None => text,
  };
  let (core, pre) = match text.split_once('-') {
    | Some((core, pre)) if is_valid_identifier_list(pre) => (core, Some(pre)),
    | Some(_) => return Err(Invalid),
    | None => (text, None),
  };

  let components = core
    .split('.')
    .map(parse_component)
    .collect::<Result<Vec<_>, _>>()?;
  let partial = match components.as_slice() {
    | [Some(major), Some(minor), Some(patch)] => {
      return Ok(Partial::Exact(Version {
        major: *major,
        minor: *minor,
        patch: *patch,
        pre: pre.map(Into::into),
      }));
    },
    | [_, _, _, _, ..] => return Err(Invalid),
    | [Some(major), Some(minor), ..] => Partial::MajorMinor(*major, *minor),
    | [Some(major), ..] => Partial::Major(*major),
    | _ => Partial::Any,
  };
  if pre.is_some() {
    return Err(Invalid);
  }
  Ok(partial)
}

/// Parses `x`, `X`, `*` (as `None`), or a number without leading zeros.
fn parse_component(text: &str) -> Result<Option<u64>, Invalid> {
  if WILDCARDS.contains(&text) {
    return Ok(None);
  }
  let is_number = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
  let has_leading_zero = text.len() > 1 && text.starts_with('0');
  if !is_number || has_leading_zero {
    return Err(Invalid);
  }
  text.parse().map(Some).map_err(|_| Invalid)
}

/// Prerelease and build metadata: non-empty dot-separated `[0-9A-Za-z-]`
/// identifiers.
fn is_valid_identifier_list(text: &str) -> bool {
  text.split('.').all(|identifier| {
    !identifier.is_empty()
      && identifier
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
  })
}

fn bump(number: u64) -> Result<u64, Invalid> {
  number.checked_add(1).ok_or(Invalid)
}

fn lowest_prerelease(major: u64, minor: u64, patch: u64) -> Version {
  Version {
    pre: Some(LOWEST_PRERELEASE.into()),
    ..Version::new(major, minor, patch)
  }
}

const fn comparator(op: Op, version: Version) -> Comparator {
  Comparator { op, version }
}

/// `^1.2.3` := `>=1.2.3 <2.0.0-0`; the first non-zero component is fixed.
fn caret(partial: Partial) -> Result<ComparatorSet, Invalid> {
  let (floor, ceiling) = match partial {
    | Partial::Any => return Ok(ComparatorSet::new()),
    | Partial::Major(major) => (
      Version::new(major, 0, 0),
      lowest_prerelease(bump(major)?, 0, 0),
    ),
    | Partial::MajorMinor(0, minor) => (
      Version::new(0, minor, 0),
      lowest_prerelease(0, bump(minor)?, 0),
    ),
    | Partial::MajorMinor(major, minor) => (
      Version::new(major, minor, 0),
      lowest_prerelease(bump(major)?, 0, 0),
    ),
    | Partial::Exact(version) => {
      let ceiling = match (version.major, version.minor) {
        | (0, 0) => lowest_prerelease(0, 0, bump(version.patch)?),
        | (0, minor) => lowest_prerelease(0, bump(minor)?, 0),
        | (major, _) => lowest_prerelease(bump(major)?, 0, 0),
      };
      (version, ceiling)
    },
  };
  Ok(vec![
    comparator(Op::GreaterOrEqual, floor),
    comparator(Op::Less, ceiling),
  ])
}

/// `~1.2.3` := `>=1.2.3 <1.3.0-0`; `~1` := `>=1.0.0 <2.0.0-0`.
fn tilde(partial: Partial) -> Result<ComparatorSet, Invalid> {
  match partial {
    | Partial::Exact(version) => {
      let ceiling = lowest_prerelease(version.major, bump(version.minor)?, 0);
      Ok(vec![
        comparator(Op::GreaterOrEqual, version),
        comparator(Op::Less, ceiling),
      ])
    },
    | partial => primitive(Op::Equal, partial),
  }
}

/// An operator applied to a possibly partial version (node-semver's
/// `replaceXRange`): `<1.2` := `<1.2.0-0`, `>1.2` := `>=1.3.0`,
/// `1.2` := `>=1.2.0 <1.3.0-0`.
fn primitive(op: Op, partial: Partial) -> Result<ComparatorSet, Invalid> {
  let (floor, next) = match partial {
    | Partial::Exact(version) => return Ok(vec![comparator(op, version)]),
    | Partial::Any if matches!(op, Op::Less | Op::Greater) => {
      return Ok(vec![comparator(Op::Less, lowest_prerelease(0, 0, 0))]);
    },
    | Partial::Any => return Ok(ComparatorSet::new()),
    | Partial::Major(major) => (Version::new(major, 0, 0), Version::new(bump(major)?, 0, 0)),
    | Partial::MajorMinor(major, minor) => (
      Version::new(major, minor, 0),
      Version::new(major, bump(minor)?, 0),
    ),
  };
  let below = |version: Version| {
    comparator(
      Op::Less,
      lowest_prerelease(version.major, version.minor, version.patch),
    )
  };
  Ok(match op {
    | Op::Equal => vec![comparator(Op::GreaterOrEqual, floor), below(next)],
    | Op::GreaterOrEqual => vec![comparator(Op::GreaterOrEqual, floor)],
    | Op::Greater => vec![comparator(Op::GreaterOrEqual, next)],
    | Op::Less => vec![below(floor)],
    | Op::LessOrEqual => vec![below(next)],
  })
}

/// `1.2.3 - 2.3.4` := `>=1.2.3 <=2.3.4`; a partial upper end excludes the
/// next release line: `1.2 - 2.3` := `>=1.2.0 <2.4.0-0`.
fn hyphen(from: Partial, to: Partial) -> Result<ComparatorSet, Invalid> {
  let lower = match from {
    | Partial::Any => None,
    | Partial::Major(major) => Some(Version::new(major, 0, 0)),
    | Partial::MajorMinor(major, minor) => Some(Version::new(major, minor, 0)),
    | Partial::Exact(version) => Some(version),
  };
  let upper = match to {
    | Partial::Exact(version) => vec![comparator(Op::LessOrEqual, version)],
    | partial => primitive(Op::LessOrEqual, partial)?,
  };
  Ok(
    lower
      .map(|version| comparator(Op::GreaterOrEqual, version))
      .into_iter()
      .chain(upper)
      .collect(),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  fn range(text: &str) -> NodeRange {
    text
      .parse()
      .unwrap_or_else(|error| panic!("{text:?} should parse: {error}"))
  }

  fn v(text: &str) -> Version {
    text.parse().unwrap()
  }

  fn includes(range_text: &str, version: &str) -> bool {
    range(range_text).matches(&v(version))
  }

  /// node-semver `range-include` fixtures that use default options.
  const INCLUDED: &[(&str, &str)] = &[
    ("1.0.0 - 2.0.0", "1.2.3"),
    ("^1.2.3+build", "1.2.3"),
    ("^1.2.3+build", "1.3.0"),
    ("1.2.3-pre+asdf - 2.4.3-pre+asdf", "1.2.3"),
    ("1.2.3-pre+asdf - 2.4.3-pre+asdf", "1.2.3-pre.2"),
    ("1.2.3-pre+asdf - 2.4.3-pre+asdf", "2.4.3-alpha"),
    ("1.2.3+asdf - 2.4.3+asdf", "1.2.3"),
    ("1.0.0", "1.0.0"),
    (">=*", "0.2.4"),
    ("", "1.0.0"),
    ("*", "1.2.3"),
    (">=1.0.0", "1.0.0"),
    (">=1.0.0", "1.0.1"),
    (">=1.0.0", "1.1.0"),
    (">1.0.0", "1.0.1"),
    (">1.0.0", "1.1.0"),
    ("<=2.0.0", "2.0.0"),
    ("<=2.0.0", "1.9999.9999"),
    ("<=2.0.0", "0.2.9"),
    ("<2.0.0", "1.9999.9999"),
    ("<2.0.0", "0.2.9"),
    (">= 1.0.0", "1.0.0"),
    (">=  1.0.0", "1.0.1"),
    (">=   1.0.0", "1.1.0"),
    ("> 1.0.0", "1.0.1"),
    (">  1.0.0", "1.1.0"),
    ("<=   2.0.0", "2.0.0"),
    ("<= 2.0.0", "1.9999.9999"),
    ("<=  2.0.0", "0.2.9"),
    ("<    2.0.0", "1.9999.9999"),
    ("<\t2.0.0", "0.2.9"),
    (">=0.1.97", "v0.1.97"),
    (">=0.1.97", "0.1.97"),
    ("0.1.20 || 1.2.4", "1.2.4"),
    (">=0.2.3 || <0.0.1", "0.0.0"),
    (">=0.2.3 || <0.0.1", "0.2.3"),
    (">=0.2.3 || <0.0.1", "0.2.4"),
    ("||", "1.3.4"),
    ("2.x.x", "2.1.3"),
    ("1.2.x", "1.2.3"),
    ("1.2.x || 2.x", "2.1.3"),
    ("1.2.x || 2.x", "1.2.3"),
    ("x", "1.2.3"),
    ("2.*.*", "2.1.3"),
    ("1.2.*", "1.2.3"),
    ("1.2.* || 2.*", "2.1.3"),
    ("1.2.* || 2.*", "1.2.3"),
    ("2", "2.1.2"),
    ("2.3", "2.3.1"),
    ("~0.0.1", "0.0.1"),
    ("~0.0.1", "0.0.2"),
    ("~x", "0.0.9"),
    ("~2", "2.0.9"),
    ("~2.4", "2.4.0"),
    ("~2.4", "2.4.5"),
    ("~>3.2.1", "3.2.2"),
    ("~1", "1.2.3"),
    ("~>1", "1.2.3"),
    ("~> 1", "1.2.3"),
    ("~1.0", "1.0.2"),
    ("~ 1.0", "1.0.2"),
    ("~ 1.0.3", "1.0.12"),
    (">=1", "1.0.0"),
    (">= 1", "1.0.0"),
    ("<1.2", "1.1.1"),
    ("< 1.2", "1.1.1"),
    ("~v0.5.4-pre", "0.5.5"),
    ("~v0.5.4-pre", "0.5.4"),
    ("=0.7.x", "0.7.2"),
    ("<=0.7.x", "0.7.2"),
    (">=0.7.x", "0.7.2"),
    ("<=0.7.x", "0.6.2"),
    ("~1.2.1 >=1.2.3", "1.2.3"),
    ("~1.2.1 =1.2.3", "1.2.3"),
    ("~1.2.1 1.2.3", "1.2.3"),
    ("~1.2.1 >=1.2.3 1.2.3", "1.2.3"),
    ("~1.2.1 1.2.3 >=1.2.3", "1.2.3"),
    (">=1.2.1 1.2.3", "1.2.3"),
    ("1.2.3 >=1.2.1", "1.2.3"),
    (">=1.2.3 >=1.2.1", "1.2.3"),
    (">=1.2.1 >=1.2.3", "1.2.3"),
    (">=1.2", "1.2.8"),
    ("^1.2.3", "1.8.1"),
    ("^0.1.2", "0.1.2"),
    ("^0.1", "0.1.2"),
    ("^0.0.1", "0.0.1"),
    ("^1.2", "1.4.2"),
    ("^1.2 ^1", "1.4.2"),
    ("^1.2.3-alpha", "1.2.3-pre"),
    ("^1.2.0-alpha", "1.2.0-pre"),
    ("^0.0.1-alpha", "0.0.1-beta"),
    ("^0.0.1-alpha", "0.0.1"),
    ("^0.1.1-alpha", "0.1.1-beta"),
    ("^x", "1.2.3"),
    ("x - 1.0.0", "0.9.7"),
    ("x - 1.x", "0.9.7"),
    ("1.0.0 - x", "1.9.7"),
    ("1.x - x", "1.9.7"),
    ("<=7.x", "7.9.9"),
    ("^0.2.3", "0.2.9"),
    ("^0.0.3", "0.0.3"),
    ("1.2 - 2.3", "2.3.9"),
    ("=v1.2.3", "1.2.3"),
    ("v=1.2.3", "1.2.3"),
  ];

  #[test]
  fn includes_node_semver_fixtures() {
    for &(range_text, version) in INCLUDED {
      assert!(
        includes(range_text, version),
        "{range_text:?} should include {version}"
      );
    }
  }

  /// node-semver `range-exclude` fixtures that use default options.
  const EXCLUDED: &[(&str, &str)] = &[
    ("1.0.0 - 2.0.0", "2.2.3"),
    ("1.2.3+asdf - 2.4.3+asdf", "1.2.3-pre.2"),
    ("1.2.3+asdf - 2.4.3+asdf", "2.4.3-alpha"),
    ("^1.2.3+build", "2.0.0"),
    ("^1.2.3+build", "1.2.0"),
    ("^1.2.3", "1.2.3-pre"),
    ("^1.2", "1.2.0-pre"),
    (">1.2", "1.3.0-beta"),
    ("<=1.2.3", "1.2.3-beta"),
    ("^1.2.3", "1.2.3-beta"),
    ("=0.7.x", "0.7.0-asdf"),
    (">=0.7.x", "0.7.0-asdf"),
    ("<=0.7.x", "0.7.0-asdf"),
    ("1.0.0", "1.0.1"),
    (">=1.0.0", "0.0.0"),
    (">=1.0.0", "0.0.1"),
    (">=1.0.0", "0.1.0"),
    (">1.0.0", "0.0.1"),
    (">1.0.0", "0.1.0"),
    ("<=2.0.0", "3.0.0"),
    ("<=2.0.0", "2.9999.9999"),
    ("<=2.0.0", "2.2.9"),
    ("<2.0.0", "2.9999.9999"),
    ("<2.0.0", "2.2.9"),
    (">=0.1.97", "v0.1.93"),
    (">=0.1.97", "0.1.93"),
    ("0.1.20 || 1.2.4", "1.2.3"),
    (">=0.2.3 || <0.0.1", "0.0.3"),
    (">=0.2.3 || <0.0.1", "0.2.2"),
    ("2.x.x", "1.1.3"),
    ("2.x.x", "3.1.3"),
    ("1.2.x", "1.3.3"),
    ("1.2.x || 2.x", "3.1.3"),
    ("1.2.x || 2.x", "1.1.3"),
    ("2.*.*", "1.1.3"),
    ("2.*.*", "3.1.3"),
    ("1.2.*", "1.3.3"),
    ("1.2.* || 2.*", "3.1.3"),
    ("1.2.* || 2.*", "1.1.3"),
    ("2", "1.1.2"),
    ("2.3", "2.4.1"),
    ("~0.0.1", "0.1.0-alpha"),
    ("~0.0.1", "0.1.0"),
    ("~2.4", "2.5.0"),
    ("~2.4", "2.3.9"),
    ("~>3.2.1", "3.3.2"),
    ("~>3.2.1", "3.2.0"),
    ("~1", "0.2.3"),
    ("~>1", "2.2.3"),
    ("~1.0", "1.1.0"),
    ("<1", "1.0.0"),
    (">=1.2", "1.1.1"),
    ("~v0.5.4-beta", "0.5.4-alpha"),
    ("=0.7.x", "0.8.2"),
    (">=0.7.x", "0.6.2"),
    ("<0.7.x", "0.7.2"),
    ("<1.2.3", "1.2.3-beta"),
    ("=1.2.3", "1.2.3-beta"),
    (">1.2", "1.2.8"),
    ("^0.0.1", "0.0.2-alpha"),
    ("^0.0.1", "0.0.2"),
    ("^1.2.3", "2.0.0-alpha"),
    ("^1.2.3", "1.2.2"),
    ("^1.2", "1.1.9"),
    ("*", "v1.2.3-foo"),
    ("^1.0.0", "2.0.0-rc1"),
    ("1 - 2", "2.0.0-pre"),
    ("1 - 2", "1.0.0-pre"),
    ("1.0 - 2", "1.0.0-pre"),
    ("1.1.x", "1.0.0-a"),
    ("1.1.x", "1.1.0-a"),
    ("1.1.x", "1.2.0-a"),
    ("1.x", "1.0.0-a"),
    ("1.x", "1.1.0-a"),
    ("1.x", "1.2.0-a"),
    (">=1.0.0 <1.1.0", "1.1.0"),
    (">=1.0.0 <1.1.0", "1.1.0-pre"),
    (">=1.0.0 <1.1.0-pre", "1.1.0-pre"),
    ("^0.2.3", "0.3.0"),
    ("^0.0.3", "0.0.4"),
    ("1.2 - 2.3", "2.4.0"),
    ("<*", "0.0.0"),
    (">*", "1.0.0"),
  ];

  #[test]
  fn excludes_node_semver_fixtures() {
    for &(range_text, version) in EXCLUDED {
      assert!(
        !includes(range_text, version),
        "{range_text:?} should exclude {version}"
      );
    }
  }

  #[test]
  fn desugars_like_node_semver() {
    for (sugar, plain) in [
      ("^0.2.3", ">=0.2.3 <0.3.0-0"),
      ("^0.0.3", ">=0.0.3 <0.0.4-0"),
      ("^1.2.3", ">=1.2.3 <2.0.0-0"),
      ("^1.2", ">=1.2.0 <2.0.0-0"),
      ("^0.2", ">=0.2.0 <0.3.0-0"),
      ("^0.0", ">=0.0.0 <0.1.0-0"),
      ("^0", ">=0.0.0 <1.0.0-0"),
      ("^1", ">=1.0.0 <2.0.0-0"),
      ("^1.2.3-beta.2", ">=1.2.3-beta.2 <2.0.0-0"),
      ("^0.0.1-beta", ">=0.0.1-beta <0.0.2-0"),
      ("~1.2.3", ">=1.2.3 <1.3.0-0"),
      ("~1.2", ">=1.2.0 <1.3.0-0"),
      ("~1", ">=1.0.0 <2.0.0-0"),
      ("~> 1", ">=1.0.0 <2.0.0-0"),
      ("~1.2.3-beta.2", ">=1.2.3-beta.2 <1.3.0-0"),
      ("1.2 - 2.3", ">=1.2.0 <2.4.0-0"),
      ("1.2.3 - 2.3.4", ">=1.2.3 <=2.3.4"),
      ("1.2.3 - 2", ">=1.2.3 <3.0.0-0"),
      ("1.2 - 2.3.4", ">=1.2.0 <=2.3.4"),
      ("x - 1.x", "<2.0.0-0"),
      ("<1.2", "<1.2.0-0"),
      (">1.2", ">=1.3.0"),
      ("<=1.2", "<1.3.0-0"),
      (">1", ">=2.0.0"),
      ("<1", "<1.0.0-0"),
      ("1.2.x", ">=1.2.0 <1.3.0-0"),
      ("1.x", ">=1.0.0 <2.0.0-0"),
      ("1", ">=1.0.0 <2.0.0-0"),
      ("1.x.3", ">=1.0.0 <2.0.0-0"),
      ("*", ""),
      ("x", ""),
      ("X", ""),
      (">=*", ""),
      ("<*", "<0.0.0-0"),
      (">*", "<0.0.0-0"),
      ("=1.2.3", "1.2.3"),
      ("v1.2.3", "1.2.3"),
      (">= 18.0.0", ">=18.0.0"),
      ("1.2.3+build", "1.2.3"),
    ] {
      assert_eq!(
        range(sugar).alternatives,
        range(plain).alternatives,
        "{sugar:?} should mean {plain:?}"
      );
    }
  }

  #[test]
  fn reads_typical_engines_fields() {
    let cases = [
      (">=18 <21", "20.11.1", true),
      (">=18 <21", "21.0.0", false),
      (">=18 <21", "17.9.1", false),
      ("^20.1", "20.19.0", true),
      ("^20.1", "20.0.0", false),
      ("~18.2.0", "18.2.9", true),
      ("~18.2.0", "18.3.0", false),
      ("18.x", "18.20.4", true),
      ("20.*", "20.0.0", true),
      ("20.*", "21.0.0", false),
      (">=16 || ^14.17", "14.17.0", true),
      (">=16 || ^14.17", "15.0.0", false),
      (">=16 || ^14.17", "22.0.0", true),
      ("v20", "20.5.0", true),
      ("=20.1.0", "20.1.0", true),
      ("=20.1.0", "20.1.1", false),
      ("20", "20.18.1", true),
      (">= 18.0.0", "18.0.0", true),
      (">=20.0.0-rc.1", "20.0.0-rc.2", true),
      (">=20.0.0-rc.1", "21.0.0-rc.1", false),
      (">=19", "24.0.0-nightly20240101abcdef", false),
    ];
    for (range_text, version, expected) in cases {
      assert_eq!(
        includes(range_text, version),
        expected,
        "{range_text:?} against {version}"
      );
    }
  }

  #[test]
  fn picks_highest_satisfying_version() {
    let versions = [
      v("16.20.2"),
      v("18.19.0"),
      v("20.11.1"),
      v("21.0.0-rc.1"),
      v("20.12.0"),
      v("22.1.0"),
    ];
    assert_eq!(
      range(">=18 <21").max_satisfying(&versions),
      Some(&v("20.12.0"))
    );
    assert_eq!(range("^18").max_satisfying(&versions), Some(&v("18.19.0")));
    assert_eq!(range("*").max_satisfying(&versions), Some(&v("22.1.0")));
    assert_eq!(range("^14").max_satisfying(&versions), None);
    assert_eq!(
      range("^21.0.0-rc.0").max_satisfying(&versions),
      Some(&v("21.0.0-rc.1"))
    );
  }

  #[test]
  fn displays_trimmed_source() {
    assert_eq!(range("  >=18   <21 ").to_string(), ">=18   <21");
    assert_eq!(range("").to_string(), "");
  }

  #[test]
  fn rejects_invalid_ranges() {
    for bad in [
      ">=abc",
      "1.2.3.4",
      "^",
      "~",
      ">=",
      "1.2.3 -",
      "- 1.2.3",
      "1 - 2 - 3",
      "01.2.3",
      "1.02.3",
      "1.2.3-",
      "1.2.3-pre..1",
      "1.2.3+",
      "1.2.x-pre",
      ">=1.2.3<2",
      "foo",
      "|",
      "v",
      "^18446744073709551615",
      "99999999999999999999",
    ] {
      assert!(
        bad.parse::<NodeRange>().is_err(),
        "{bad:?} should not parse"
      );
    }
  }

  #[test]
  fn error_names_bad_text() {
    let error = ">=18 || >=abc".parse::<NodeRange>().unwrap_err();
    assert_eq!(
      error.to_string(),
      "'>=18 || >=abc' is not a valid semver range ('>=abc' is not a valid comparator)"
    );
  }
}
