//! Finding installed versions.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use super::NvmDir;
use crate::version::Version;

/// Which runtime a version belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flavor {
  Node,
  /// io.js, the 2015 fork. rnm only reads existing io.js installs.
  Iojs,
}

/// One installed version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledVersion {
  pub flavor: Flavor,
  pub version: Version,
  /// The version directory, for example `<root>/versions/node/v20.11.1`.
  pub path: PathBuf,
}

impl InstalledVersion {
  /// The directory that holds `node` (`bin/` on unix, the version root on
  /// Windows).
  #[must_use]
  pub fn bin_dir(&self) -> PathBuf {
    bin_dir(&self.path)
  }
}

/// Formats like nvm: `v20.11.1`, or `iojs-v3.3.1` for io.js.
impl fmt::Display for InstalledVersion {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self.flavor {
      | Flavor::Node => self.version.fmt(f),
      | Flavor::Iojs => write!(f, "iojs-{}", self.version),
    }
  }
}

/// The directory that holds the `node` binary inside a version directory.
#[must_use]
pub fn bin_dir(version_dir: &Path) -> PathBuf {
  if cfg!(windows) {
    version_dir.to_path_buf()
  } else {
    version_dir.join("bin")
  }
}

/// The `node` binary inside a version directory.
#[must_use]
pub fn node_binary(version_dir: &Path) -> PathBuf {
  bin_dir(version_dir).join(if cfg!(windows) { "node.exe" } else { "node" })
}

/// nvm's installed check (`[ -x "$dir/bin/node" ]`).
fn has_node_binary(version_dir: &Path) -> bool {
  is_executable(&node_binary(version_dir))
}

/// The path is a file that can be executed. On unix this needs an execute
/// bit; on Windows, existence is enough.
#[must_use]
pub fn is_executable(path: &Path) -> bool {
  let Ok(metadata) = std::fs::metadata(path) else {
    return false;
  };
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
  }
  #[cfg(not(unix))]
  {
    metadata.is_file()
  }
}

impl NvmDir {
  /// The installed version whose directory is `path`, if `path` is a version
  /// directory in this tree with a usable `node` binary.
  #[must_use]
  pub fn installed_at(&self, path: &Path) -> Option<InstalledVersion> {
    let version: Version = path.file_name()?.to_str()?.parse().ok()?;
    let parent = path.parent()?;
    let flavor = if parent == self.iojs_versions_dir() {
      Flavor::Iojs
    } else if parent == self.node_versions_dir()
      || (parent == self.root && (version.major, version.minor) < (0, 12))
    {
      Flavor::Node
    } else {
      return None;
    };
    has_node_binary(path).then(|| InstalledVersion {
      flavor,
      version,
      path: path.to_path_buf(),
    })
  }

  /// Lists installed versions, sorted from oldest to newest (io.js after
  /// node for equal numbers).
  ///
  /// Scans `versions/node`, `versions/io.js`, and the legacy root for
  /// directories named `v<version>`. Unlike `nvm ls`, a directory only counts
  /// when it contains a usable `node` binary, so a half-finished install is
  /// never selected. Missing directories are treated as empty.
  ///
  /// # Errors
  ///
  /// Returns an error if a directory exists but cannot be read.
  pub fn installed_versions(&self) -> io::Result<Vec<InstalledVersion>> {
    let mut found = Vec::new();
    // The legacy root only holds node < 0.12; anything newer there (or the
    // `versions` dir itself) is not a legacy install.
    let legacy = |v: &Version| (v.major, v.minor) < (0, 12);
    scan(
      &self.node_versions_dir(),
      Flavor::Node,
      |_| true,
      &mut found,
    )?;
    scan(&self.root, Flavor::Node, legacy, &mut found)?;
    scan(
      &self.iojs_versions_dir(),
      Flavor::Iojs,
      |_| true,
      &mut found,
    )?;
    found.sort_by(|a, b| a.version.cmp(&b.version).then(a.flavor.cmp(&b.flavor)));
    found.dedup_by(|a, b| a.version == b.version && a.flavor == b.flavor);
    Ok(found)
  }
}

fn scan(
  dir: &Path,
  flavor: Flavor,
  accept: impl Fn(&Version) -> bool,
  found: &mut Vec<InstalledVersion>,
) -> io::Result<()> {
  let entries = match std::fs::read_dir(dir) {
    | Ok(entries) => entries,
    | Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
    | Err(e) => return Err(e),
  };
  for entry in entries {
    let entry = entry?;
    let name = entry.file_name();
    let Some(name) = name.to_str() else { continue };
    if !name.starts_with('v') {
      continue;
    }
    let Ok(version) = name.parse::<Version>() else {
      continue;
    };
    let path = entry.path();
    if accept(&version) && has_node_binary(&path) {
      found.push(InstalledVersion {
        flavor,
        version,
        path,
      });
    }
  }
  Ok(())
}

#[cfg(test)]
pub(super) mod tests {
  use super::*;

  /// Creates a fake install with an executable `node` binary.
  pub fn fake_install(dir: &Path) {
    let binary = node_binary(dir);
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
  }

  #[test]
  fn lists_all_layouts_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = NvmDir::new(tmp.path());
    for (flavor, v) in [
      (Flavor::Node, "20.11.1"),
      (Flavor::Node, "18.19.0"),
      (Flavor::Node, "0.10.48"),
      (Flavor::Iojs, "3.3.1"),
      (Flavor::Node, "24.0.0-rc.1"),
    ] {
      fake_install(&dir.version_dir(flavor, &v.parse().unwrap()));
    }
    let listed: Vec<String> = dir
      .installed_versions()
      .unwrap()
      .iter()
      .map(ToString::to_string)
      .collect();
    assert_eq!(
      listed,
      [
        "v0.10.48",
        "iojs-v3.3.1",
        "v18.19.0",
        "v20.11.1",
        "v24.0.0-rc.1"
      ]
    );
  }

  #[test]
  fn skips_incomplete_and_foreign_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = NvmDir::new(tmp.path());
    let node = dir.node_versions_dir();
    std::fs::create_dir_all(node.join("v22.0.0")).unwrap(); // no binary
    std::fs::create_dir_all(node.join("not-a-version")).unwrap();
    std::fs::create_dir_all(node.join("vfoo")).unwrap();
    fake_install(&node.join("v20.0.0"));
    // A modern version in the legacy root is not a legacy install.
    fake_install(&tmp.path().join("v19.0.0"));
    let listed: Vec<String> = dir
      .installed_versions()
      .unwrap()
      .iter()
      .map(ToString::to_string)
      .collect();
    assert_eq!(listed, ["v20.0.0"]);
  }

  #[cfg(unix)]
  #[test]
  fn requires_executable_binary() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = NvmDir::new(tmp.path());
    let path = dir.node_versions_dir().join("v20.0.0");
    fake_install(&path);
    std::fs::set_permissions(node_binary(&path), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(dir.installed_versions().unwrap(), Vec::new());
  }

  #[test]
  fn installed_at_maps_paths_back() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = NvmDir::new(tmp.path());
    let node = dir.version_dir(Flavor::Node, &"20.0.0".parse().unwrap());
    let iojs = dir.version_dir(Flavor::Iojs, &"3.3.1".parse().unwrap());
    let legacy = dir.version_dir(Flavor::Node, &"0.10.48".parse().unwrap());
    for path in [&node, &iojs, &legacy] {
      fake_install(path);
    }
    assert_eq!(dir.installed_at(&node).unwrap().to_string(), "v20.0.0");
    assert_eq!(dir.installed_at(&iojs).unwrap().to_string(), "iojs-v3.3.1");
    assert_eq!(dir.installed_at(&legacy).unwrap().to_string(), "v0.10.48");
    assert_eq!(
      dir.installed_at(&tmp.path().join("elsewhere").join("v20.0.0")),
      None
    );
    assert_eq!(
      dir.installed_at(&dir.node_versions_dir().join("v21.0.0")),
      None
    );
  }

  #[test]
  fn missing_tree_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = NvmDir::new(tmp.path().join("nope"));
    assert_eq!(dir.installed_versions().unwrap(), Vec::new());
  }
}
