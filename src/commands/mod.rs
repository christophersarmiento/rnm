//! Command implementations. Each command writes normal output to `out` and
//! returns a [`Failure`] with nvm's exit code on error. `main.rs` only parses
//! arguments and maps failures to exit codes.

mod alias;
mod colors;
mod current;
mod env;
mod exec;
mod install;
mod listing;
mod ls;
mod ls_remote;
mod uninstall;
mod use_version;
mod which;

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use alias::{alias, unalias};
pub use current::current;
pub use env::env;
pub use exec::{ExecOptions, exec};
pub use install::{InstallOptions, Progress, install};
pub use ls::{LsOptions, ls};
pub use ls_remote::{LsRemoteOptions, ls_remote};
pub use uninstall::uninstall;
pub use use_version::{UseOptions, use_version};
pub use which::which;

use crate::http::{Client, Mirror};
use crate::multishell::{self, Multishells, Session};
use crate::nvm_dir::{AliasError, NODE_EXECUTABLE, NvmDir, global_modules_dir, is_executable};
use crate::remote::{RemoteIndex, load_index, refresh_lts_aliases};

/// What nvm prints for "no version is active".
pub const NONE_LABEL: &str = "none";

/// The alias a new shell session starts with.
pub const DEFAULT_ALIAS: &str = "default";

/// What nvm prints for a Node.js found outside its tree.
pub const SYSTEM_LABEL: &str = "system";

/// A failed command: the message for stderr and the process exit code. An
/// empty message means the command's output already explains the failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
  pub code: u8,
  pub message: String,
}

/// nvm's exit codes, kept for scripts that check them.
pub mod exit {
  /// General error.
  pub const ERROR: u8 = 1;
  /// The requested version is not installed or not available.
  pub const NOT_AVAILABLE: u8 = 3;
  /// An alias chain loops.
  pub const ALIAS_LOOP: u8 = 8;
  /// No version given and none found in a version file, or usage error.
  pub const NO_VERSION: u8 = 127;
  /// An option that is not supported in this combination.
  pub const INVALID_OPTION: u8 = 55;
}

impl Failure {
  #[must_use]
  pub fn new(code: u8, message: impl Into<String>) -> Self {
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

impl From<AliasError> for Failure {
  fn from(e: AliasError) -> Self {
    Self::new(exit::ERROR, e.to_string())
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

  /// Reads an environment variable as text; empty counts as unset.
  #[must_use]
  pub fn var_string(&self, key: &str) -> Option<String> {
    self.var(key).map(|v| v.to_string_lossy().into_owned())
  }

  /// `dir` is inside the install tree or a session link, so it belongs to
  /// rnm (or nvm), not to the system.
  #[must_use]
  pub fn is_managed(&self, dir: &Path) -> bool {
    dir.starts_with(self.nvm_dir.root()) || dir.starts_with(self.multishells.dir())
  }

  /// The first `node` on `PATH` that is not managed by rnm: not inside the
  /// install tree and not inside a session link.
  #[must_use]
  pub fn system_node(&self) -> Option<PathBuf> {
    self
      .path
      .iter()
      .filter(|dir| !self.is_managed(dir))
      .map(|dir| dir.join(NODE_EXECUTABLE))
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
  let package = global_modules_dir(version_dir)
    .join("npm")
    .join("package.json");
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

/// Loads the mirror's version index (revalidating the cached copy) and
/// refreshes nvm's LTS aliases from it. Warnings go to `err`.
///
/// # Errors
///
/// Exit code 1 when the mirror settings are invalid or the index cannot be
/// loaded.
pub(crate) fn remote_index(ctx: &Context, err: &mut dyn Write) -> Result<RemoteIndex, Failure> {
  let failure = |e: &dyn fmt::Display| Failure::new(exit::ERROR, e.to_string());
  let mirror = Mirror::from_env(|key| ctx.var(key)).map_err(|e| failure(&e))?;
  let client = Client::for_mirror(&mirror);
  let loaded = load_index(&client, &mirror, &ctx.nvm_dir.cache_dir()).map_err(|e| failure(&e))?;
  for warning in &loaded.warnings {
    say(err, warning);
  }
  if let Err(e) = refresh_lts_aliases(&ctx.nvm_dir.aliases(), &loaded.index) {
    say(
      err,
      format_args!("warning: cannot update the LTS aliases: {e}"),
    );
  }
  Ok(loaded.index)
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
    let dir = global_modules_dir(tmp.path()).join("npm");
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
