//! Command implementations. Each command writes normal output to `out` and
//! returns a [`Failure`] with nvm's exit code on error. `main.rs` only parses
//! arguments and maps failures to exit codes.

mod current;
mod env;
mod use_version;

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use current::current;
pub use env::env;
pub use use_version::{UseOptions, use_version};

use crate::multishell::{self, Multishells, Session};
use crate::nvm_dir::{NvmDir, is_executable};

/// A failed command: the message for stderr and the process exit code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
  pub code: i32,
  pub message: String,
}

/// nvm's exit codes, kept for scripts that check them.
pub mod exit {
  /// General error.
  pub const ERROR: i32 = 1;
  /// The requested version is not installed or not available.
  pub const NOT_AVAILABLE: i32 = 3;
  /// An alias chain loops.
  pub const ALIAS_LOOP: i32 = 8;
  /// No version given and none found in a version file, or usage error.
  pub const NO_VERSION: i32 = 127;
}

impl Failure {
  #[must_use]
  pub fn new(code: i32, message: impl Into<String>) -> Self {
    Self {
      code,
      message: message.into(),
    }
  }
}

impl fmt::Display for Failure {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.message)
  }
}

impl From<std::io::Error> for Failure {
  fn from(e: std::io::Error) -> Self {
    Self::new(exit::ERROR, e.to_string())
  }
}

/// Reads an environment variable.
pub type EnvReader = Box<dyn Fn(&str) -> Option<OsString>>;

/// Everything a command reads from the environment, so commands can be
/// tested without touching the real process environment.
pub struct Context {
  pub nvm_dir: NvmDir,
  pub multishells: Multishells,
  /// The logical working directory (`$PWD` when it is valid).
  pub cwd: PathBuf,
  /// `PATH`, split.
  pub path: Vec<PathBuf>,
  /// `RNM_MULTISHELL_PATH`.
  pub session: Option<Session>,
  /// Reads other environment variables.
  pub var: EnvReader,
}

impl Context {
  /// Builds a context from the process environment.
  ///
  /// # Errors
  ///
  /// Fails if the install tree cannot be located.
  pub fn from_env() -> Result<Self, Failure> {
    let var = |key: &str| std::env::var_os(key);
    let nvm_dir = NvmDir::locate_with(var).map_err(|e| Failure::new(exit::ERROR, e.to_string()))?;
    let multishells = Multishells::locate_with(var, &nvm_dir);
    let cwd = logical_cwd(var("PWD"))?;
    let path = var("PATH")
      .map(|p| std::env::split_paths(&p).collect())
      .unwrap_or_default();
    let session = var(multishell::ENV_VAR)
      .filter(|v| !v.is_empty())
      .map(Session::new);
    Ok(Self {
      nvm_dir,
      multishells,
      cwd,
      path,
      session,
      var: Box::new(var),
    })
  }

  /// Reads an environment variable; empty counts as unset.
  #[must_use]
  pub fn var(&self, key: &str) -> Option<OsString> {
    (self.var)(key).filter(|v| !v.is_empty())
  }

  /// The first `node` on `PATH` that is not managed by rnm: not inside the
  /// install tree and not inside a session link.
  #[must_use]
  pub fn system_node(&self) -> Option<PathBuf> {
    let managed =
      |dir: &Path| dir.starts_with(self.nvm_dir.root()) || dir.starts_with(self.multishells.dir());
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    self
      .path
      .iter()
      .filter(|dir| !managed(dir))
      .map(|dir| dir.join(name))
      .find(|p| is_executable(p))
  }
}

/// `$PWD` if it names the current directory (keeping symlinked paths, as nvm
/// does), otherwise the canonical current directory.
fn logical_cwd(pwd: Option<OsString>) -> Result<PathBuf, Failure> {
  let actual = std::env::current_dir()?;
  if let Some(pwd) = pwd.map(PathBuf::from)
    && pwd.is_absolute()
    && same_file(&pwd, &actual)
  {
    return Ok(pwd);
  }
  Ok(actual)
}

fn same_file(a: &Path, b: &Path) -> bool {
  match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
    | (Ok(a), Ok(b)) => a == b,
    | _ => false,
  }
}

/// The npm version bundled in a version directory, read from npm's
/// `package.json` without running anything.
#[must_use]
pub fn npm_version(version_dir: &Path) -> Option<String> {
  let package = if cfg!(windows) {
    version_dir
      .join("node_modules")
      .join("npm")
      .join("package.json")
  } else {
    version_dir
      .join("lib")
      .join("node_modules")
      .join("npm")
      .join("package.json")
  };
  let text = std::fs::read_to_string(package).ok()?;
  json_string_field(&text, "version")
}

/// The first `"key": "value"` string field in a JSON text. Enough for npm's
/// `package.json`, where `version` is a top-level string.
fn json_string_field(text: &str, key: &str) -> Option<String> {
  let needle = format!("\"{key}\"");
  let after_key = &text[text.find(&needle)? + needle.len()..];
  let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
  let value = after_colon.strip_prefix('"')?;
  Some(value[..value.find('"')?].to_owned())
}

/// Writes a line, ignoring broken pipes (`rnm current | head`).
pub(crate) fn say(out: &mut dyn Write, line: impl fmt::Display) {
  let _ = writeln!(out, "{line}");
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reads_npm_version() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = if cfg!(windows) {
      tmp.path().join("node_modules").join("npm")
    } else {
      tmp.path().join("lib").join("node_modules").join("npm")
    };
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
      dir.join("package.json"),
      "{\n  \"name\": \"npm\",\n  \"version\" : \"10.8.2\",\n}",
    )
    .unwrap();
    assert_eq!(npm_version(tmp.path()), Some("10.8.2".to_owned()));
    assert_eq!(npm_version(&tmp.path().join("nope")), None);
  }

  #[test]
  fn json_field() {
    assert_eq!(
      json_string_field(r#"{"version":"1.2.3"}"#, "version"),
      Some("1.2.3".to_owned())
    );
    assert_eq!(json_string_field(r#"{"version": 3}"#, "version"), None);
    assert_eq!(json_string_field("{}", "version"), None);
  }
}
