//! Command implementations. Each command writes normal output to `out` and
//! returns a [`Failure`] with nvm's exit code on error. `main.rs` only parses
//! arguments and maps failures to exit codes.

mod alias;
mod colors;
mod current;
mod default_version;
mod doctor;
mod env;
mod exec;
mod import;
mod install;
mod listing;
mod ls;
mod ls_remote;
mod packages;
mod uninstall;
mod use_version;
mod which;

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use alias::{alias, unalias};
pub use current::current;
pub use default_version::{Persistence, default_version};
pub use doctor::doctor;
pub use env::{EnvOptions, env};
pub use exec::{ExecOptions, exec};
pub use import::{ImportMode, ImportOptions, import_nvm_windows};
pub use install::{InstallOptions, Progress, install};
pub use ls::{LsOptions, ls};
pub use ls_remote::{LsRemoteOptions, Order, ls_remote};
pub use packages::{DEFAULT_PACKAGES_FILE, install_latest_npm_command, reinstall_packages_command};
pub use uninstall::uninstall;
pub use use_version::{UseOptions, use_version};
pub use which::which;

use crate::config::{Config, env_flag};
use crate::http::{Mirror, Network};
use crate::multishell::{self, Multishells, Session};
use crate::nvm_dir::{AliasError, NODE_EXECUTABLE, NvmDir, global_modules_dir, is_executable};
use crate::platform::Platform;
use crate::remote::{RemoteIndex, load_index, refresh_lts_aliases};
use crate::shell::{Inferred, ShellKind};
use crate::version::VersionFileKind;

/// Turn on `package.json` as a version file, highest precedence first.
const RESOLVE_ENGINES_VARS: [&str; 2] = ["RNM_RESOLVE_ENGINES", "FNM_RESOLVE_ENGINES"];

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
  /// `reinstall-packages` from the active version.
  pub const SAME_AS_CURRENT: u8 = 2;
  /// The requested version is not installed or not available.
  pub const NOT_AVAILABLE: u8 = 3;
  /// `install --reinstall-packages-from` the version being installed.
  pub const SAME_AS_INSTALLING: u8 = 4;
  /// The `--reinstall-packages-from` version is not installed.
  pub const SOURCE_NOT_INSTALLED: u8 = 5;
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
  /// `config.toml`.
  pub config: Config,
}

impl Context {
  /// Builds a context from the process environment.
  ///
  /// # Errors
  ///
  /// Fails if the install tree cannot be located or `config.toml` cannot
  /// be read.
  pub fn from_env() -> Result<Self, Failure> {
    let var = |key: &str| std::env::var_os(key);
    let config = Config::load(var).map_err(|e| Failure::new(exit::ERROR, e.to_string()))?;
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
      config,
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

  /// The version files to look for: `.nvmrc` and `.node-version`, plus
  /// `package.json` when `RNM_RESOLVE_ENGINES` (or fnm's
  /// `FNM_RESOLVE_ENGINES`) or `version_files.package_json` turns it on.
  #[must_use]
  pub fn version_file_kinds(&self) -> &'static [VersionFileKind] {
    let reads_package_json = env_flag(|key| self.var(key), &RESOLVE_ENGINES_VARS)
      .or(self.config.version_files.package_json)
      .unwrap_or(false);
    if reads_package_json {
      VersionFileKind::WITH_PACKAGE_JSON
    } else {
      VersionFileKind::DEFAULT
    }
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

/// The mirrors and HTTP client from `config.toml` and the environment.
/// Warnings go to `err`.
///
/// # Errors
///
/// Exit code 1 when the settings are invalid.
pub(crate) fn network(ctx: &Context, err: &mut dyn Write) -> Result<Network, Failure> {
  let needs_unofficial_builds = Platform::detect().is_ok_and(|p| p.needs_unofficial_builds());
  let network = Network::from_settings(&ctx.config, |key| ctx.var(key), needs_unofficial_builds)
    .map_err(|e| Failure::new(exit::ERROR, e.to_string()))?;
  for warning in &network.warnings {
    say(err, warning);
  }
  Ok(network)
}

/// The mirror's version index and the network settings that loaded it.
pub(crate) struct Remote {
  pub index: RemoteIndex,
  pub network: Network,
  /// The position of the mirror that served the index.
  pub served_by: usize,
}

impl Remote {
  /// The mirrors to download from: the one that served the index, then
  /// the ones after it. Earlier mirrors already failed to serve the index.
  pub fn download_mirrors(&self) -> &[Mirror] {
    &self.network.mirrors[self.served_by..]
  }
}

/// Loads the mirror's version index (revalidating the cached copy) and
/// refreshes nvm's LTS aliases from it. Warnings go to `err`.
///
/// # Errors
///
/// Exit code 1 when the mirror settings are invalid or the index cannot be
/// loaded.
pub(crate) fn remote_index(ctx: &Context, err: &mut dyn Write) -> Result<Remote, Failure> {
  let network = network(ctx, err)?;
  let loaded = load_index(&network.client, &network.mirrors, &ctx.nvm_dir.cache_dir())
    .map_err(|e| Failure::new(exit::ERROR, e.to_string()))?;
  for warning in &loaded.warnings {
    say(err, warning);
  }
  if let Err(e) = refresh_lts_aliases(&ctx.nvm_dir.aliases(), &loaded.index) {
    say(
      err,
      format_args!("warning: cannot update the LTS aliases: {e}"),
    );
  }
  Ok(Remote {
    index: loaded.index,
    network,
    served_by: loaded.mirror,
  })
}

/// The calling shell, found in the process tree, for commands whose
/// `--shell` was omitted.
///
/// # Errors
///
/// Exit code 1 when the shell is not supported or cannot be found.
pub fn infer_shell() -> Result<ShellKind, Failure> {
  match ShellKind::detect() {
    | Inferred::Supported(shell) => Ok(shell),
    | Inferred::Unsupported(name) => Err(Failure::new(
      exit::ERROR,
      format!(
        "rnm does not support the '{name}' shell yet; supported shells: {}",
        ShellKind::names()
      ),
    )),
    | Inferred::Unknown => Err(Failure::new(
      exit::ERROR,
      format!(
        "cannot detect your shell; pass it with --shell ({})",
        ShellKind::names()
      ),
    )),
  }
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
