//! The install tree rnm shares with nvm.
//!
//! On unix this is nvm's `$NVM_DIR` (see `docs/research/nvm.md` §4). On
//! Windows nvm does not exist, so rnm uses `%LOCALAPPDATA%\rnm` with the same
//! layout, except that `node.exe` sits in the version root instead of `bin/`.
//!
//! ```text
//! <root>/
//!   versions/node/vX.Y.Z/     node >= 0.12
//!   versions/io.js/vX.Y.Z/    io.js (no "iojs-" prefix in the dir name)
//!   vX.Y.Z/                   legacy layout for node < 0.12
//!   alias/<name>              one-line alias files
//!   alias/lts/<codename>      LTS aliases; alias/lts/* points at the newest
//!   .cache/                   downloads and install locks
//! ```

mod alias;
mod installed;
mod resolve;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

pub use alias::{AliasError, Aliases};
pub use installed::{Flavor, InstalledVersion};
pub use resolve::{ResolveError, Resolved};

use crate::version::Version;

/// The root of the install tree and the paths inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvmDir {
  root: PathBuf,
}

/// Why the install tree's location could not be determined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocateError {
  /// None of the variables that locate the tree are set (`HOME` on unix,
  /// `LOCALAPPDATA` on Windows).
  NoHome,
}

impl fmt::Display for LocateError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::NoHome if cfg!(windows) => {
        f.write_str("cannot find the install directory: set RNM_DIR or LOCALAPPDATA")
      },
      | Self::NoHome => f.write_str("cannot find the install directory: set NVM_DIR or HOME"),
    }
  }
}

impl std::error::Error for LocateError {}

impl NvmDir {
  /// Uses `root` as the install tree.
  #[must_use]
  pub fn new(root: impl Into<PathBuf>) -> Self {
    Self { root: root.into() }
  }

  /// Locates the install tree from the process environment.
  ///
  /// # Errors
  ///
  /// See [`NvmDir::locate_with`].
  pub fn locate() -> Result<Self, LocateError> {
    Self::locate_with(|key| std::env::var_os(key))
  }

  /// Locates the install tree using `env` to read environment variables.
  ///
  /// Unix: `RNM_DIR`, then `NVM_DIR`, then `$XDG_CONFIG_HOME/nvm` when
  /// `XDG_CONFIG_HOME` is set, then `$HOME/.nvm` (nvm's `install.sh` rules).
  ///
  /// Windows: `RNM_DIR`, then `%LOCALAPPDATA%\rnm`. `NVM_DIR` is ignored,
  /// because nvm does not run on Windows and `NVM_HOME` belongs to
  /// nvm-windows.
  ///
  /// Empty variables count as unset. Trailing separators are removed, as nvm
  /// does.
  ///
  /// # Errors
  ///
  /// Returns [`LocateError::NoHome`] if no variable gives a location.
  pub fn locate_with(env: impl Fn(&str) -> Option<OsString>) -> Result<Self, LocateError> {
    let var = |key: &str| {
      env(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    };

    let root = if cfg!(windows) {
      var("RNM_DIR").or_else(|| var("LOCALAPPDATA").map(|dir| dir.join("rnm")))
    } else {
      var("RNM_DIR")
        .or_else(|| var("NVM_DIR"))
        .or_else(|| var("XDG_CONFIG_HOME").map(|dir| dir.join("nvm")))
        .or_else(|| var("HOME").map(|dir| dir.join(".nvm")))
    };
    root
      .map(|root| Self::new(strip_trailing_separators(root)))
      .ok_or(LocateError::NoHome)
  }

  /// The root of the tree.
  #[must_use]
  pub fn root(&self) -> &Path {
    &self.root
  }

  /// `versions/node`.
  #[must_use]
  pub fn node_versions_dir(&self) -> PathBuf {
    self.root.join("versions").join("node")
  }

  /// `versions/io.js`.
  #[must_use]
  pub fn iojs_versions_dir(&self) -> PathBuf {
    self.root.join("versions").join("io.js")
  }

  /// `alias`.
  #[must_use]
  pub fn alias_dir(&self) -> PathBuf {
    self.root.join("alias")
  }

  /// `.cache`.
  #[must_use]
  pub fn cache_dir(&self) -> PathBuf {
    self.root.join(".cache")
  }

  /// The directory a version is (or would be) installed in, following
  /// `nvm_version_path`: io.js under `versions/io.js`, node before 0.12 in the
  /// root (legacy layout), everything else under `versions/node`.
  #[must_use]
  pub fn version_dir(&self, flavor: Flavor, version: &Version) -> PathBuf {
    let name = version.to_string();
    match flavor {
      | Flavor::Iojs => self.iojs_versions_dir().join(name),
      | Flavor::Node if (version.major, version.minor) < (0, 12) => self.root.join(name),
      | Flavor::Node => self.node_versions_dir().join(name),
    }
  }

  /// Opens the alias store.
  #[must_use]
  pub fn aliases(&self) -> Aliases {
    Aliases::new(self.alias_dir())
  }
}

/// Removes trailing path separators, but keeps a bare root such as `/`.
fn strip_trailing_separators(path: PathBuf) -> PathBuf {
  let text = path.to_string_lossy();
  let is_separator = |c: char| std::path::is_separator(c);
  let trimmed = text.trim_end_matches(is_separator);
  if trimmed.len() == text.len() || trimmed.is_empty() || trimmed.ends_with(':') {
    return path;
  }
  PathBuf::from(trimmed)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;

  fn locate(vars: &[(&str, &str)]) -> Result<NvmDir, LocateError> {
    let vars: HashMap<String, OsString> = vars
      .iter()
      .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
      .collect();
    NvmDir::locate_with(|key| vars.get(key).cloned())
  }

  #[cfg(unix)]
  #[test]
  fn locates_like_nvm_on_unix() {
    let root = |vars: &[(&str, &str)]| locate(vars).unwrap().root().to_path_buf();
    assert_eq!(root(&[("HOME", "/home/u")]), PathBuf::from("/home/u/.nvm"));
    assert_eq!(
      root(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/xdg")]),
      PathBuf::from("/xdg/nvm")
    );
    assert_eq!(
      root(&[
        ("HOME", "/home/u"),
        ("XDG_CONFIG_HOME", "/xdg"),
        ("NVM_DIR", "/opt/nvm//")
      ]),
      PathBuf::from("/opt/nvm")
    );
    assert_eq!(
      root(&[("NVM_DIR", "/opt/nvm"), ("RNM_DIR", "/opt/rnm")]),
      PathBuf::from("/opt/rnm")
    );
    assert_eq!(
      root(&[("NVM_DIR", ""), ("HOME", "/home/u")]),
      PathBuf::from("/home/u/.nvm")
    );
    assert_eq!(locate(&[]), Err(LocateError::NoHome));
  }

  #[cfg(windows)]
  #[test]
  fn locates_in_local_app_data_on_windows() {
    let root = |vars: &[(&str, &str)]| locate(vars).unwrap().root().to_path_buf();
    assert_eq!(
      root(&[
        ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
        ("NVM_DIR", r"C:\nvm")
      ]),
      PathBuf::from(r"C:\Users\u\AppData\Local\rnm")
    );
    assert_eq!(root(&[("RNM_DIR", r"D:\rnm\")]), PathBuf::from(r"D:\rnm"));
    assert_eq!(locate(&[("HOME", r"C:\Users\u")]), Err(LocateError::NoHome));
  }

  #[test]
  fn keeps_bare_root() {
    assert_eq!(
      strip_trailing_separators(PathBuf::from("/")),
      PathBuf::from("/")
    );
  }

  #[test]
  fn version_dirs_follow_nvm_layout() {
    let dir = NvmDir::new("/n");
    let v = |s: &str| s.parse::<Version>().unwrap();
    assert_eq!(
      dir.version_dir(Flavor::Node, &v("20.11.1")),
      Path::new("/n")
        .join("versions")
        .join("node")
        .join("v20.11.1")
    );
    assert_eq!(
      dir.version_dir(Flavor::Node, &v("0.12.0")),
      Path::new("/n")
        .join("versions")
        .join("node")
        .join("v0.12.0")
    );
    assert_eq!(
      dir.version_dir(Flavor::Node, &v("0.10.48")),
      Path::new("/n").join("v0.10.48")
    );
    assert_eq!(
      dir.version_dir(Flavor::Iojs, &v("3.3.1")),
      Path::new("/n")
        .join("versions")
        .join("io.js")
        .join("v3.3.1")
    );
  }
}
