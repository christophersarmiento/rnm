//! The mirror's `index.json`: every release with its date, platform files,
//! bundled npm, and LTS codename (`docs/research/ecosystem.md` §4.1).

use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::version::Version;

/// One release listed in `index.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
  pub version: Version,
  /// The release date, `YYYY-MM-DD`.
  pub date: String,
  /// Platform file IDs such as `linux-x64` or `osx-arm64-tar`. They are not
  /// file names; see `docs/research/ecosystem.md` §4.2.
  pub files: Vec<String>,
  /// The bundled npm version. Missing in very old releases.
  pub npm: Option<String>,
  /// The LTS codename as published, for example `Jod`. `None` for releases
  /// outside an LTS line, and for codenames that are unsafe as alias file
  /// names (see [`is_valid_codename`]).
  pub lts: Option<String>,
  /// The release fixes security issues.
  pub security: bool,
}

impl Release {
  /// The LTS codename in lowercase, as nvm uses it in `lts/<codename>`.
  #[must_use]
  pub fn lts_name(&self) -> Option<String> {
    self.lts.as_deref().map(str::to_ascii_lowercase)
  }

  fn from_raw(raw: RawRelease) -> Option<Self> {
    let lts = match raw.lts {
      | Some(LtsField::Codename(name)) if is_valid_codename(&name) => Some(name),
      | _ => None,
    };
    Some(Self {
      version: raw.version.parse().ok()?,
      date: raw.date,
      files: raw.files,
      npm: raw.npm,
      lts,
      security: raw.security,
    })
  }
}

/// All releases from one mirror, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteIndex {
  releases: Vec<Release>,
}

/// The `lts` field is a codename string, or `false` outside LTS lines. Any
/// other value also counts as "not LTS".
#[derive(Deserialize)]
#[serde(untagged)]
enum LtsField {
  Codename(String),
  NotLts(IgnoredAny),
}

#[derive(Deserialize)]
struct RawRelease {
  version: String,
  #[serde(default)]
  date: String,
  #[serde(default)]
  files: Vec<String>,
  npm: Option<String>,
  lts: Option<LtsField>,
  #[serde(default)]
  security: bool,
}

impl RemoteIndex {
  /// Parses `index.json`.
  ///
  /// Entries whose version does not parse are skipped, so one odd entry on
  /// a mirror does not hide every other release. The result is sorted newest
  /// first, whatever order the mirror used.
  ///
  /// # Errors
  ///
  /// Fails if the text is not a JSON array of release objects.
  pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
    let raw: Vec<RawRelease> = serde_json::from_str(json)?;
    let mut releases: Vec<Release> = raw.into_iter().filter_map(Release::from_raw).collect();
    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(Self { releases })
  }

  /// Builds an index from releases in any order.
  #[must_use]
  pub fn from_releases(mut releases: Vec<Release>) -> Self {
    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Self { releases }
  }

  /// All releases, newest first.
  #[must_use]
  pub fn releases(&self) -> &[Release] {
    &self.releases
  }

  /// The newest release of each LTS line, newest line first, as
  /// `(lowercase codename, release)`.
  #[must_use]
  pub fn lts_lines(&self) -> Vec<(String, &Release)> {
    let mut lines: Vec<(String, &Release)> = Vec::new();
    for release in &self.releases {
      if let Some(name) = release.lts_name()
        && !lines.iter().any(|(seen, _)| *seen == name)
      {
        lines.push((name, release));
      }
    }
    lines
  }
}

/// nvm's guard for codenames that become `alias/lts/<codename>` files:
/// lowercase matches `^[a-z0-9][a-z0-9._-]*$`.
#[must_use]
pub fn is_valid_codename(name: &str) -> bool {
  let is_allowed = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-');
  name
    .bytes()
    .next()
    .is_some_and(|first| first.is_ascii_alphanumeric())
    && name.bytes().all(is_allowed)
}

#[cfg(test)]
mod tests {
  use super::*;

  const SAMPLE: &str = r#"[
    {"version":"v22.20.0","date":"2025-09-24","files":["linux-x64","osx-arm64-tar"],
     "npm":"10.9.3","v8":"12.4","lts":"Jod","security":false},
    {"version":"v24.9.0","date":"2025-09-25","files":["linux-x64"],"npm":"11.6.0",
     "lts":false,"security":true},
    {"version":"v22.19.0","date":"2025-08-28","files":[],"lts":"Jod","security":false},
    {"version":"v20.19.5","date":"2025-09-03","files":[],"lts":"Iron","security":false},
    {"version":"not-a-version","date":"2025-01-01","files":[],"lts":false},
    {"version":"v18.0.0","date":"2022-04-19","files":[],"lts":"bad name","security":false},
    {"version":"v0.1.14","date":"2011-08-26","files":["src"],"v8":"1.3.15.0","lts":false}
  ]"#;

  fn versions(index: &RemoteIndex) -> Vec<String> {
    index
      .releases()
      .iter()
      .map(|r| r.version.to_string())
      .collect()
  }

  #[test]
  fn parses_and_sorts_newest_first() {
    let index = RemoteIndex::parse(SAMPLE).unwrap();
    assert_eq!(
      versions(&index),
      [
        "v24.9.0", "v22.20.0", "v22.19.0", "v20.19.5", "v18.0.0", "v0.1.14"
      ]
    );
    let newest = &index.releases()[0];
    assert_eq!(newest.date, "2025-09-25");
    assert_eq!(newest.files, ["linux-x64"]);
    assert_eq!(newest.npm.as_deref(), Some("11.6.0"));
    assert_eq!(newest.lts, None);
    assert!(newest.security);
    let oldest = index.releases().last().unwrap();
    assert_eq!(oldest.npm, None);
    assert!(!oldest.security);
  }

  #[test]
  fn keeps_only_safe_codenames() {
    let index = RemoteIndex::parse(SAMPLE).unwrap();
    let lts = |v: &str| {
      index
        .releases()
        .iter()
        .find(|r| r.version.to_string() == v)
        .unwrap()
        .lts
        .clone()
    };
    assert_eq!(lts("v22.20.0").as_deref(), Some("Jod"));
    assert_eq!(lts("v18.0.0"), None);
  }

  #[test]
  fn lists_lts_lines_newest_first() {
    let index = RemoteIndex::parse(SAMPLE).unwrap();
    let lines: Vec<(String, String)> = index
      .lts_lines()
      .into_iter()
      .map(|(name, r)| (name, r.version.to_string()))
      .collect();
    assert_eq!(
      lines,
      [
        ("jod".to_owned(), "v22.20.0".to_owned()),
        ("iron".to_owned(), "v20.19.5".to_owned())
      ]
    );
  }

  #[test]
  fn rejects_non_arrays() {
    assert!(RemoteIndex::parse("{}").is_err());
    assert!(RemoteIndex::parse("[{\"date\":\"x\"}]").is_err());
    assert_eq!(RemoteIndex::parse("[]").unwrap(), RemoteIndex::default());
  }

  #[test]
  fn codename_guard_matches_nvm() {
    for good in ["jod", "Jod", "a", "9", "a.b_c-d"] {
      assert!(is_valid_codename(good), "{good:?}");
    }
    for bad in ["", "-a", ".a", "_a", "a b", "a/b", "*", "a\n", "é"] {
      assert!(!is_valid_codename(bad), "{bad:?}");
    }
  }
}
