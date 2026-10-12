//! Per-shell-session links ("multishell", the design fnm uses).
//!
//! `rnm env` creates one link per shell session and puts it on `PATH` once.
//! `rnm use` only re-points the link, so the shell's environment never has to
//! change again. On unix the link is a symlink, replaced atomically with a
//! rename. On Windows it is a directory junction, which needs no admin rights
//! or Developer Mode.
//!
//! Links are named `<shell pid>_<nanoseconds>`. `rnm env` removes links whose
//! shell is gone. If a link is removed while its shell is still running (for
//! example after PID reuse), the next `rnm use` simply recreates it.
//!
//! `system` is a link to an empty directory (`.system`), so `PATH` lookups
//! fall through to the next entry.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::nvm_dir::NvmDir;

/// The environment variable that holds this shell session's link.
pub const ENV_VAR: &str = "RNM_MULTISHELL_PATH";

/// The name of the empty directory that `system` links point to.
const SYSTEM_DIR: &str = ".system";

/// The directory that holds all session links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Multishells {
  dir: PathBuf,
}

impl Multishells {
  /// Uses `dir` to hold session links.
  #[must_use]
  pub fn new(dir: impl Into<PathBuf>) -> Self {
    Self { dir: dir.into() }
  }

  /// Picks the links directory.
  ///
  /// Unix: `$XDG_RUNTIME_DIR/rnm/multishells` (cleared at logout), else
  /// `$XDG_STATE_HOME/rnm/multishells`, else
  /// `~/.local/state/rnm/multishells`. Nothing is put inside `$NVM_DIR`, so
  /// nvm's tree stays untouched.
  ///
  /// Windows: `<install root>\multishells`, because the Windows tree is
  /// rnm's own.
  #[must_use]
  pub fn locate_with(env: impl Fn(&str) -> Option<OsString>, nvm_dir: &NvmDir) -> Self {
    if cfg!(windows) {
      return Self::new(nvm_dir.root().join("multishells"));
    }
    let var = |key: &str| {
      env(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    };
    let base = var("XDG_RUNTIME_DIR")
      .or_else(|| var("XDG_STATE_HOME"))
      .or_else(|| var("HOME").map(|home| home.join(".local").join("state")))
      .unwrap_or_else(|| nvm_dir.root().join(".rnm-state"));
    Self::new(base.join("rnm").join("multishells"))
  }

  /// The links directory.
  #[must_use]
  pub fn dir(&self) -> &Path {
    &self.dir
  }

  /// Creates a new session link owned by `owner_pid`, pointing at `target`
  /// (a version directory) or at the `system` placeholder when `None`.
  ///
  /// # Errors
  ///
  /// Returns an error if the directory or link cannot be created.
  pub fn create(&self, owner_pid: u32, target: Option<&Path>) -> io::Result<Session> {
    std::fs::create_dir_all(&self.dir)?;
    let nanos = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .map_or(0, |d| d.as_nanos());
    let session = Session::new(self.dir.join(format!("{owner_pid}_{nanos}")));
    session.point_to(target)?;
    Ok(session)
  }

  /// Removes links whose owning shell is no longer running. Returns how many
  /// were removed. Entries that are not session links are left alone.
  ///
  /// # Errors
  ///
  /// Returns an error if the directory exists but cannot be read. Failures to
  /// remove single links are ignored; the next run tries again.
  pub fn collect_garbage(&self, is_alive: impl Fn(u32) -> bool) -> io::Result<usize> {
    let entries = match std::fs::read_dir(&self.dir) {
      | Ok(entries) => entries,
      | Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
      | Err(e) => return Err(e),
    };
    let mut removed = 0;
    for entry in entries {
      let session = Session::new(entry?.path());
      if let Some(pid) = session.owner_pid()
        && !is_alive(pid)
        && remove_link(&session.path).is_ok()
      {
        removed += 1;
      }
    }
    Ok(removed)
  }
}

/// One shell session's link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
  path: PathBuf,
}

impl Session {
  /// Wraps an existing link path (for example from `RNM_MULTISHELL_PATH`).
  #[must_use]
  pub fn new(path: impl Into<PathBuf>) -> Self {
    Self { path: path.into() }
  }

  /// The link path.
  #[must_use]
  pub fn path(&self) -> &Path {
    &self.path
  }

  /// The directory to put on `PATH`: `<link>/bin` on unix, the link itself on
  /// Windows (where `node.exe` sits in the version root).
  #[must_use]
  pub fn bin_dir(&self) -> PathBuf {
    if cfg!(windows) {
      self.path.clone()
    } else {
      self.path.join("bin")
    }
  }

  /// The headers directory, exported as `NVM_INC`.
  #[must_use]
  pub fn include_dir(&self) -> PathBuf {
    self.path.join("include").join("node")
  }

  /// The PID of the shell that owns this link, from its name.
  #[must_use]
  pub fn owner_pid(&self) -> Option<u32> {
    let name = self.path.file_name()?.to_str()?;
    let (pid, nanos) = name.split_once('_')?;
    nanos.bytes().all(|b| b.is_ascii_digit()).then_some(())?;
    pid.parse().ok()
  }

  /// Where the link points, or `None` if the link does not exist.
  /// [`Target::System`] for the `system` placeholder.
  ///
  /// # Errors
  ///
  /// Returns an error if the link exists but cannot be read.
  pub fn target(&self) -> io::Result<Option<Target>> {
    match read_link(&self.path) {
      | Ok(target) if target.file_name().is_some_and(|name| name == SYSTEM_DIR) => {
        Ok(Some(Target::System))
      },
      | Ok(target) => Ok(Some(Target::Version(target))),
      | Err(e)
        if matches!(
          e.kind(),
          io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
        ) =>
      {
        Ok(None)
      },
      | Err(e) => Err(e),
    }
  }

  /// Points the link at `target` (a version directory), or at the `system`
  /// placeholder when `None`. Creates the link if it is missing.
  ///
  /// # Errors
  ///
  /// Returns an error if the link cannot be replaced.
  pub fn point_to(&self, target: Option<&Path>) -> io::Result<()> {
    let parent = self
      .path
      .parent()
      .ok_or_else(|| io::Error::other("link has no parent"))?;
    let target = match target {
      | Some(target) => target.to_path_buf(),
      | None => {
        let system = parent.join(SYSTEM_DIR);
        std::fs::create_dir_all(&system)?;
        system
      },
    };
    replace_link(&target, &self.path)
  }
}

/// What a session link points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
  /// A version directory.
  Version(PathBuf),
  /// The `system` placeholder.
  System,
}

#[cfg(unix)]
fn read_link(link: &Path) -> io::Result<PathBuf> {
  std::fs::read_link(link)
}

#[cfg(windows)]
fn read_link(link: &Path) -> io::Result<PathBuf> {
  junction::get_target(link)
}

/// Atomically replaces `link` with a symlink to `target`.
#[cfg(unix)]
fn replace_link(target: &Path, link: &Path) -> io::Result<()> {
  let name = link
    .file_name()
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_default();
  let tmp = link.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
  let _ = std::fs::remove_file(&tmp);
  std::os::unix::fs::symlink(target, &tmp)?;
  std::fs::rename(&tmp, link).inspect_err(|_| {
    let _ = std::fs::remove_file(&tmp);
  })
}

/// Replaces `link` with a junction to `target`. Junctions cannot be swapped
/// atomically, so the old one is removed first.
#[cfg(windows)]
fn replace_link(target: &Path, link: &Path) -> io::Result<()> {
  match remove_link(link) {
    | Ok(()) => {},
    | Err(e) if e.kind() == io::ErrorKind::NotFound => {},
    | Err(e) => return Err(e),
  }
  junction::create(target, link)
}

#[cfg(unix)]
fn remove_link(link: &Path) -> io::Result<()> {
  std::fs::remove_file(link)
}

/// Removing a junction with `remove_dir` deletes only the junction, never the
/// target's contents.
#[cfg(windows)]
fn remove_link(link: &Path) -> io::Result<()> {
  std::fs::remove_dir(link)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn version_dir(root: &Path, name: &str) -> PathBuf {
    let dir = root.join("versions").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("marker"), name).unwrap();
    dir
  }

  #[test]
  fn create_and_repoint() {
    let tmp = tempfile::tempdir().unwrap();
    let shells = Multishells::new(tmp.path().join("ms"));
    let v18 = version_dir(tmp.path(), "v18");
    let v20 = version_dir(tmp.path(), "v20");

    let session = shells.create(4242, Some(&v18)).unwrap();
    assert_eq!(session.owner_pid(), Some(4242));
    assert_eq!(
      session.target().unwrap(),
      Some(Target::Version(v18.clone()))
    );
    assert_eq!(
      std::fs::read_to_string(session.path().join("marker")).unwrap(),
      "v18"
    );

    session.point_to(Some(&v20)).unwrap();
    assert_eq!(session.target().unwrap(), Some(Target::Version(v20)));
    assert_eq!(
      std::fs::read_to_string(session.path().join("marker")).unwrap(),
      "v20"
    );

    session.point_to(None).unwrap();
    assert_eq!(session.target().unwrap(), Some(Target::System));
    assert!(!session.path().join("marker").exists());

    // The version directories are untouched.
    assert!(v18.join("marker").is_file());
  }

  #[test]
  fn missing_link_is_recreated() {
    let tmp = tempfile::tempdir().unwrap();
    let v18 = version_dir(tmp.path(), "v18");
    let session = Session::new(tmp.path().join("ms").join("1_2"));
    assert_eq!(session.target().unwrap(), None);
    std::fs::create_dir_all(tmp.path().join("ms")).unwrap();
    session.point_to(Some(&v18)).unwrap();
    assert_eq!(session.target().unwrap(), Some(Target::Version(v18)));
  }

  #[test]
  fn garbage_collection_removes_dead_sessions_only() {
    let tmp = tempfile::tempdir().unwrap();
    let shells = Multishells::new(tmp.path().join("ms"));
    let v18 = version_dir(tmp.path(), "v18");
    let alive = shells.create(100, Some(&v18)).unwrap();
    let dead = shells.create(200, None).unwrap();
    std::fs::write(shells.dir().join("unrelated"), "").unwrap();

    assert_eq!(shells.collect_garbage(|pid| pid == 100).unwrap(), 1);
    assert!(alive.target().unwrap().is_some());
    assert_eq!(dead.target().unwrap(), None);
    assert!(shells.dir().join(SYSTEM_DIR).is_dir());
    assert!(shells.dir().join("unrelated").is_file());
    assert!(v18.join("marker").is_file());
  }

  #[test]
  fn owner_pid_parsing() {
    assert_eq!(Session::new("/x/123_456").owner_pid(), Some(123));
    assert_eq!(Session::new("/x/.system").owner_pid(), None);
    assert_eq!(Session::new("/x/123_abc").owner_pid(), None);
    assert_eq!(Session::new("/x/.123_4.tmp-9").owner_pid(), None);
  }

  #[cfg(unix)]
  #[test]
  fn locates_state_dir() {
    let nvm = NvmDir::new("/n");
    let locate = |vars: &[(&str, &str)]| {
      let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
      Multishells::locate_with(
        |key| {
          vars
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| OsString::from(v))
        },
        &nvm,
      )
      .dir()
      .to_path_buf()
    };
    assert_eq!(
      locate(&[("HOME", "/h")]),
      Path::new("/h/.local/state/rnm/multishells")
    );
    assert_eq!(
      locate(&[("HOME", "/h"), ("XDG_STATE_HOME", "/s")]),
      Path::new("/s/rnm/multishells")
    );
    assert_eq!(
      locate(&[
        ("HOME", "/h"),
        ("XDG_STATE_HOME", "/s"),
        ("XDG_RUNTIME_DIR", "/run/user/1")
      ]),
      Path::new("/run/user/1/rnm/multishells")
    );
  }
}
