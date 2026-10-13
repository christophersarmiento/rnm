//! Remote resolution: turning a [`VersionSpec`] into a release from the
//! mirror's index (nvm's `nvm_remote_version`; `docs/research/nvm.md` §2.3).

use std::fmt;

use super::index::{Release, RemoteIndex};
use crate::version::{LtsSpec, VersionSpec};

/// Why a spec has no remote release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteResolveError {
  /// No release matches.
  NotFound(VersionSpec),
  /// The spec only has a local meaning (`system`, `current`, a user alias)
  /// or names io.js, which rnm does not install.
  NotRemote(VersionSpec),
  /// `lts/-N` asks for more LTS lines than the index has.
  LtsOffsetTooLarge(u32),
}

impl fmt::Display for RemoteResolveError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::NotFound(spec) => write!(f, "version '{spec}' not found on the mirror"),
      | Self::NotRemote(spec @ VersionSpec::Iojs(_)) => {
        write!(f, "'{spec}': rnm does not install io.js")
      },
      | Self::NotRemote(spec) => write!(f, "'{spec}' cannot be resolved on the mirror"),
      | Self::LtsOffsetTooLarge(n) => {
        write!(f, "lts/-{n}: that many LTS releases do not exist yet")
      },
    }
  }
}

impl std::error::Error for RemoteResolveError {}

impl RemoteIndex {
  /// The newest release that satisfies `spec`.
  ///
  /// - Version numbers pick the newest match (`18` → newest `v18.x.y`).
  /// - `node` and `stable` pick the newest release (for `0.x`, only even
  ///   minors); `unstable` picks the newest odd `0.x`.
  /// - `lts/*` picks the newest LTS release, `lts/<name>` the newest of that
  ///   line (case-insensitive), and `lts/-N` the newest of the Nth line
  ///   before the newest. LTS comes from the index's `lts` field, never from
  ///   the major version's parity.
  ///
  /// # Errors
  ///
  /// See [`RemoteResolveError`].
  pub fn resolve(&self, spec: &VersionSpec) -> Result<&Release, RemoteResolveError> {
    let newest = |accept: &dyn Fn(&Release) -> bool| {
      self
        .releases()
        .iter()
        .find(|release| accept(release))
        .ok_or_else(|| RemoteResolveError::NotFound(spec.clone()))
    };
    match spec {
      | VersionSpec::Version(req) => newest(&|r| req.matches(&r.version)),
      | VersionSpec::Range(range) => newest(&|r| range.matches(&r.version)),
      | VersionSpec::Node | VersionSpec::Stable => newest(&|r| r.version.is_stable()),
      | VersionSpec::Unstable => newest(&|r| r.version.is_unstable()),
      | VersionSpec::Lts(LtsSpec::Latest) => newest(&|r| r.lts.is_some()),
      | VersionSpec::Lts(LtsSpec::Named(name)) => {
        newest(&|r| r.lts_name().as_deref() == Some(&**name))
      },
      | VersionSpec::Lts(LtsSpec::Offset(n)) => {
        let lines = self.lts_lines();
        usize::try_from(*n)
          .ok()
          .and_then(|n| lines.get(n))
          .map(|(_, release)| *release)
          .ok_or(RemoteResolveError::LtsOffsetTooLarge(*n))
      },
      | VersionSpec::Iojs(_)
      | VersionSpec::System
      | VersionSpec::Current
      | VersionSpec::Alias(_) => Err(RemoteResolveError::NotRemote(spec.clone())),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn release(version: &str, lts: Option<&str>) -> Release {
    Release {
      version: version.parse().unwrap(),
      date: String::new(),
      files: Vec::new(),
      npm: None,
      lts: lts.map(str::to_owned),
      security: false,
    }
  }

  fn index() -> RemoteIndex {
    RemoteIndex::from_releases(vec![
      release("0.10.48", None),
      release("0.11.16", None),
      release("18.20.8", Some("Hydrogen")),
      release("18.3.0", None),
      release("20.19.4", Some("Iron")),
      release("20.19.5", Some("Iron")),
      release("22.20.0", Some("Jod")),
      release("24.9.0", None),
      release("25.0.0-rc.1", None),
    ])
  }

  fn resolve(spec: &str) -> String {
    match index().resolve(&spec.parse().unwrap()) {
      | Ok(release) => release.version.to_string(),
      | Err(e) => format!("error: {e}"),
    }
  }

  #[test]
  fn versions_pick_newest_match() {
    assert_eq!(resolve("18"), "v18.20.8");
    assert_eq!(resolve("v18.3"), "v18.3.0");
    assert_eq!(resolve("20.19.4"), "v20.19.4");
    assert_eq!(resolve("1"), "error: version 'v1' not found on the mirror");
    assert_eq!(resolve("25"), "v25.0.0-rc.1");
  }

  #[test]
  fn implicit_aliases() {
    assert_eq!(resolve("node"), "v25.0.0-rc.1");
    assert_eq!(resolve("stable"), "v25.0.0-rc.1");
    assert_eq!(resolve("unstable"), "v0.11.16");
  }

  #[test]
  fn lts_uses_the_index_field() {
    assert_eq!(resolve("lts/*"), "v22.20.0");
    assert_eq!(resolve("lts/iron"), "v20.19.5");
    assert_eq!(resolve("lts/-1"), "v20.19.5");
    assert_eq!(resolve("lts/-2"), "v18.20.8");
    assert_eq!(
      resolve("lts/-3"),
      "error: lts/-3: that many LTS releases do not exist yet"
    );
    assert_eq!(
      resolve("lts/argon"),
      "error: version 'lts/argon' not found on the mirror"
    );
  }

  #[test]
  fn local_only_specs() {
    for spec in ["system", "current", "default", "iojs"] {
      assert!(resolve(spec).starts_with("error: '"), "{spec}");
    }
  }
}
