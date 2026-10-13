//! Local resolution: turning a [`VersionSpec`] into an installed version,
//! without the network (nvm's `nvm_version`; `docs/research/nvm.md` §2.2).

use std::fmt;
use std::io;

use super::NvmDir;
use super::alias::AliasError;
use super::installed::{Flavor, InstalledVersion};
use crate::version::{LTS_PREFIX, LtsSpec, SpecError, Version, VersionReq, VersionSpec};

/// The result of resolving a spec locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
  /// An installed version.
  Installed(InstalledVersion),
  /// `system`, possibly reached through an alias. Whether a system Node.js
  /// exists is checked by the caller, which knows `PATH`.
  System,
  /// `current`, possibly reached through an alias. The caller knows which
  /// version is active.
  Current,
}

/// Why a spec could not be resolved locally.
#[derive(Debug)]
pub enum ResolveError {
  /// Nothing installed matches.
  NotInstalled(VersionSpec),
  /// The alias (or LTS alias such as `lts/iron`) does not exist.
  AliasNotFound(String),
  /// Following aliases led back to an alias already seen. Holds the chain,
  /// ending with the repeated name.
  AliasCycle(Vec<String>),
  /// An alias file's target is not a valid spec.
  BadAliasTarget {
    alias: String,
    target: String,
    error: SpecError,
  },
  /// `lts/-N` asks for more LTS lines than there are LTS alias files.
  LtsOffsetTooLarge(u32),
  /// Reading aliases failed.
  Alias(AliasError),
  /// Listing installed versions failed.
  Io(io::Error),
}

impl fmt::Display for ResolveError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::NotInstalled(spec) => write!(f, "version '{spec}' is not installed"),
      | Self::AliasNotFound(name) if name.starts_with(LTS_PREFIX) => {
        write!(
          f,
          "LTS alias '{name}' not found; run 'rnm ls-remote --lts' to refresh LTS aliases"
        )
      },
      | Self::AliasNotFound(name) => write!(f, "alias '{name}' does not exist"),
      | Self::AliasCycle(chain) => write!(f, "alias loop: {}", chain.join(" -> ")),
      | Self::BadAliasTarget {
        alias,
        target,
        error,
      } => {
        write!(
          f,
          "alias '{alias}' points to '{target}', which is invalid: {error}"
        )
      },
      | Self::LtsOffsetTooLarge(n) => {
        write!(f, "lts/-{n}: that many LTS releases do not exist yet")
      },
      | Self::Alias(e) => e.fmt(f),
      | Self::Io(e) => write!(f, "cannot list installed versions: {e}"),
    }
  }
}

impl std::error::Error for ResolveError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    match self {
      | Self::BadAliasTarget { error, .. } => Some(error),
      | Self::Alias(e) => Some(e),
      | Self::Io(e) => Some(e),
      | _ => None,
    }
  }
}

impl From<AliasError> for ResolveError {
  fn from(e: AliasError) -> Self {
    Self::Alias(e)
  }
}

impl NvmDir {
  /// Resolves `spec` against installed versions and aliases.
  ///
  /// - Version numbers pick the highest installed match (`18` → newest
  ///   `v18.x.y`). An exact version that is missing for node falls back to
  ///   an io.js install of the same number, as nvm does.
  /// - `node` and `stable` pick the newest installed node (for `0.x`, only
  ///   even minors); `unstable` picks the newest odd `0.x`.
  /// - `lts/*`, `lts/<name>`, and user aliases follow alias files, with loop
  ///   detection. `lts/-N` counts back through the LTS alias files in
  ///   alphabetical order, which is release order.
  /// - An alias file named like an exact version (`alias/v20.0.0`) shadows
  ///   that version, as in nvm.
  /// - `system` and `current` are returned as markers for the caller.
  ///
  /// # Errors
  ///
  /// See [`ResolveError`].
  pub fn resolve(&self, spec: &VersionSpec) -> Result<Resolved, ResolveError> {
    let installed = self.installed_versions().map_err(ResolveError::Io)?;
    self.resolve_among(spec, &installed)
  }

  /// [`NvmDir::resolve`], choosing among `candidates` instead of the
  /// installed versions (for example installed plus cached versions for
  /// `install --offline`). Aliases still come from this tree.
  ///
  /// # Errors
  ///
  /// See [`ResolveError`].
  pub fn resolve_among(
    &self,
    spec: &VersionSpec,
    candidates: &[InstalledVersion],
  ) -> Result<Resolved, ResolveError> {
    Resolver {
      dir: self,
      installed: candidates,
      chain: Vec::new(),
    }
    .resolve(spec)
  }
}

struct Resolver<'a> {
  dir: &'a NvmDir,
  installed: &'a [InstalledVersion],
  /// Aliases followed so far, for loop detection and error messages.
  chain: Vec<String>,
}

impl Resolver<'_> {
  fn resolve(&mut self, spec: &VersionSpec) -> Result<Resolved, ResolveError> {
    let not_installed = || ResolveError::NotInstalled(spec.clone());
    match spec {
      | VersionSpec::Version(req) => {
        if let VersionReq::Exact(version) = req
          && self.dir.aliases().get(&version.to_string())?.is_some()
        {
          return self.follow(&version.to_string());
        }
        self
          .newest(Flavor::Node, |v| req.matches(v))
          .or_else(|| match req {
            | VersionReq::Exact(_) => self.newest(Flavor::Iojs, |v| req.matches(v)),
            | _ => None,
          })
          .ok_or_else(not_installed)
      },
      | VersionSpec::Node | VersionSpec::Stable => self
        .newest(Flavor::Node, Version::is_stable)
        .ok_or_else(not_installed),
      | VersionSpec::Unstable => self
        .newest(Flavor::Node, Version::is_unstable)
        .ok_or_else(not_installed),
      | VersionSpec::Iojs(req) => self
        .newest(Flavor::Iojs, |v| {
          req.as_ref().is_none_or(|req| req.matches(v))
        })
        .ok_or_else(not_installed),
      | VersionSpec::System => Ok(Resolved::System),
      | VersionSpec::Current => Ok(Resolved::Current),
      | VersionSpec::Lts(LtsSpec::Latest | LtsSpec::Named(_)) => self.follow(&spec.to_string()),
      | VersionSpec::Lts(LtsSpec::Offset(n)) => {
        let names = self.dir.aliases().lts_names()?;
        let newest = names.len().checked_sub(1);
        let nth_before_newest = usize::try_from(*n)
          .ok()
          .and_then(|n| newest?.checked_sub(n));
        match nth_before_newest {
          | Some(index) => self.follow(&format!("{LTS_PREFIX}{}", names[index])),
          | None => Err(ResolveError::LtsOffsetTooLarge(*n)),
        }
      },
      | VersionSpec::Alias(name) => self.follow(name),
      | VersionSpec::Range(range) => self
        .newest(Flavor::Node, |v| range.matches(v))
        .ok_or_else(not_installed),
    }
  }

  fn newest(&self, flavor: Flavor, accept: impl Fn(&Version) -> bool) -> Option<Resolved> {
    self
      .installed
      .iter()
      .filter(|i| i.flavor == flavor && accept(&i.version))
      .max_by(|a, b| a.version.cmp(&b.version))
      .cloned()
      .map(Resolved::Installed)
  }

  fn follow(&mut self, name: &str) -> Result<Resolved, ResolveError> {
    if self.chain.iter().any(|seen| seen == name) {
      let mut chain = std::mem::take(&mut self.chain);
      chain.push(name.to_owned());
      return Err(ResolveError::AliasCycle(chain));
    }
    self.chain.push(name.to_owned());
    let target = self
      .dir
      .aliases()
      .get(name)?
      .ok_or_else(|| ResolveError::AliasNotFound(name.to_owned()))?;
    let spec = target
      .parse::<VersionSpec>()
      .map_err(|error| ResolveError::BadAliasTarget {
        alias: name.to_owned(),
        target: target.clone(),
        error,
      })?;
    self.resolve(&spec)
  }
}

#[cfg(test)]
mod tests {
  use super::super::installed::tests::fake_install;
  use super::*;

  struct Tree {
    _tmp: tempfile::TempDir,
    dir: NvmDir,
  }

  impl Tree {
    fn new(versions: &[&str]) -> Self {
      let tmp = tempfile::tempdir().unwrap();
      let dir = NvmDir::new(tmp.path());
      for v in versions {
        let (flavor, v) = match v.strip_prefix("iojs-") {
          | Some(v) => (Flavor::Iojs, v),
          | None => (Flavor::Node, *v),
        };
        fake_install(&dir.version_dir(flavor, &v.parse().unwrap()));
      }
      Self { _tmp: tmp, dir }
    }

    fn alias(&self, name: &str, target: &str) -> &Self {
      self.dir.aliases().set(name, target).unwrap();
      self
    }

    /// Resolves and formats like `nvm version`.
    fn resolve(&self, spec: &str) -> String {
      match self.dir.resolve(&spec.parse().unwrap()) {
        | Ok(Resolved::Installed(i)) => i.to_string(),
        | Ok(Resolved::System) => "system".to_owned(),
        | Ok(Resolved::Current) => "current".to_owned(),
        | Err(e) => format!("error: {e}"),
      }
    }
  }

  #[test]
  fn partial_versions_pick_newest_match() {
    let t = Tree::new(&[
      "18.19.0",
      "18.20.4",
      "18.3.0",
      "20.11.1",
      "1.8.4-x.1",
      "0.10.48",
    ]);
    assert_eq!(t.resolve("18"), "v18.20.4");
    assert_eq!(t.resolve("v18.3"), "v18.3.0");
    assert_eq!(t.resolve("18.19.0"), "v18.19.0");
    assert_eq!(t.resolve("20"), "v20.11.1");
    assert_eq!(t.resolve("0.10"), "v0.10.48");
    assert!(
      t.resolve("19")
        .starts_with("error: version 'v19' is not installed")
    );
    assert!(t.resolve("18.19.1").starts_with("error:"));
    // `1` must not match v18 or v20.
    assert_eq!(t.resolve("1"), "v1.8.4-x.1");
  }

  #[test]
  fn release_beats_its_prerelease() {
    let t = Tree::new(&["24.0.0-rc.1", "24.0.0", "24.0.0-rc.2"]);
    assert_eq!(t.resolve("24"), "v24.0.0");
    assert_eq!(t.resolve("24.0.0-rc.2"), "v24.0.0-rc.2");
  }

  #[test]
  fn node_stable_unstable() {
    let t = Tree::new(&["0.10.48", "0.11.16", "18.20.4", "iojs-3.3.1"]);
    assert_eq!(t.resolve("node"), "v18.20.4");
    assert_eq!(t.resolve("stable"), "v18.20.4");
    assert_eq!(t.resolve("unstable"), "v0.11.16");

    let old = Tree::new(&["0.10.48", "0.11.16"]);
    assert_eq!(old.resolve("node"), "v0.10.48");
  }

  #[test]
  fn iojs() {
    let t = Tree::new(&["iojs-2.5.0", "iojs-3.3.1", "18.20.4"]);
    assert_eq!(t.resolve("iojs"), "iojs-v3.3.1");
    assert_eq!(t.resolve("iojs-v2"), "iojs-v2.5.0");
    // An exact number missing for node falls back to io.js.
    assert_eq!(t.resolve("3.3.1"), "iojs-v3.3.1");
    // Partial versions do not fall back.
    assert!(t.resolve("3").starts_with("error:"));
  }

  #[test]
  fn aliases_and_chains() {
    let t = Tree::new(&["18.20.4", "20.11.1"]);
    t.alias("default", "work")
      .alias("work", "18")
      .alias("sys", "system")
      .alias("cur", "current");
    assert_eq!(t.resolve("default"), "v18.20.4");
    assert_eq!(t.resolve("sys"), "system");
    assert_eq!(t.resolve("cur"), "current");
    assert_eq!(
      t.resolve("missing"),
      "error: alias 'missing' does not exist"
    );
  }

  #[test]
  fn alias_cycles_are_detected() {
    let t = Tree::new(&["18.20.4"]);
    t.alias("a", "b")
      .alias("b", "c")
      .alias("c", "a")
      .alias("self", "self");
    assert_eq!(t.resolve("a"), "error: alias loop: a -> b -> c -> a");
    assert_eq!(t.resolve("self"), "error: alias loop: self -> self");
  }

  #[test]
  fn bad_alias_target() {
    let t = Tree::new(&[]);
    t.alias("broken", "18..1");
    assert!(
      t.resolve("broken")
        .starts_with("error: alias 'broken' points to '18..1'")
    );
  }

  #[test]
  fn alias_shadows_exact_version() {
    let t = Tree::new(&["18.20.4", "20.11.1"]);
    t.alias("v20.11.1", "18");
    assert_eq!(t.resolve("20.11.1"), "v18.20.4");
    assert_eq!(t.resolve("20"), "v20.11.1");
  }

  #[test]
  fn lts_aliases() {
    let t = Tree::new(&["18.20.4", "20.19.0", "22.20.0"]);
    t.alias("lts/hydrogen", "v18.20.4")
      .alias("lts/iron", "v20.19.0")
      .alias("lts/jod", "v22.20.0")
      .alias("lts/*", "lts/jod")
      .alias("default", "lts/*");
    assert_eq!(t.resolve("lts/*"), "v22.20.0");
    assert_eq!(t.resolve("default"), "v22.20.0");
    assert_eq!(t.resolve("lts/iron"), "v20.19.0");
    assert_eq!(t.resolve("lts/-1"), "v20.19.0");
    assert_eq!(t.resolve("lts/-2"), "v18.20.4");
    assert_eq!(
      t.resolve("lts/-3"),
      "error: lts/-3: that many LTS releases do not exist yet"
    );
    assert!(
      t.resolve("lts/argon")
        .starts_with("error: LTS alias 'lts/argon' not found")
    );
  }

  #[test]
  fn lts_alias_to_missing_version() {
    let t = Tree::new(&["20.19.0"]);
    t.alias("lts/jod", "v22.20.0").alias("lts/*", "lts/jod");
    assert_eq!(
      t.resolve("lts/*"),
      "error: version 'v22.20.0' is not installed"
    );
  }

  #[test]
  fn system_and_current_pass_through() {
    let t = Tree::new(&[]);
    assert_eq!(t.resolve("system"), "system");
    assert_eq!(t.resolve("current"), "current");
  }
}
