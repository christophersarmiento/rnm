//! The user's settings file, `config.toml`.
//!
//! rnm reads one file, from the user's own configuration directory, never
//! from a project: a cloned repository must not be able to change the
//! mirror or capture credentials (`docs/research/ecosystem.md` §1.4).
//!
//! - Unix: `$XDG_CONFIG_HOME/rnm/config.toml`, else
//!   `~/.config/rnm/config.toml`.
//! - Windows: `%APPDATA%\rnm\config.toml`.
//! - `RNM_CONFIG` names another file.
//!
//! Precedence, lowest first: built-in defaults, this file, environment
//! variables (`RNM_*`, then nvm's and fnm's names), command-line flags.
//!
//! ```toml
//! [mirror]
//! url = "https://artifactory.corp/artifactory/nodejs-dist/"
//! fallback = ["https://nodejs.org/dist/"]
//! unofficial_builds = true      # musl, riscv64, loong64
//!
//! [[auth]]
//! prefix = "https://artifactory.corp/artifactory/"
//! value_env = "ARTIFACTORY_TOKEN"
//! scheme = "Bearer"
//!
//! [http]
//! netrc = true
//! ca_file = "/etc/corp/ca.pem"
//!
//! [version_files]
//! package_json = true           # devEngines.runtime, engines.node
//! ```

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Names a settings file to read instead of the default one.
pub const CONFIG_ENV: &str = "RNM_CONFIG";

const DIR_NAME: &str = "rnm";
const FILE_NAME: &str = "config.toml";

/// The parsed settings file. Every field is optional; a missing file is the
/// same as an empty one.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
  #[serde(default)]
  pub mirror: MirrorSettings,
  /// Credentials, each sent only to URLs under its `prefix`.
  #[serde(default)]
  pub auth: Vec<AuthEntry>,
  #[serde(default)]
  pub http: HttpSettings,
  #[serde(default)]
  pub version_files: VersionFileSettings,
  /// The file these settings came from, for messages.
  #[serde(skip)]
  pub path: Option<PathBuf>,
}

/// `[mirror]`: where Node.js is downloaded from.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorSettings {
  /// The base URL, laid out like `https://nodejs.org/dist`.
  pub url: Option<String>,
  /// Mirrors to try, in order, when the main one cannot be reached or
  /// lacks a file.
  #[serde(default)]
  pub fallback: Vec<String>,
  /// On systems with no official binaries (musl libc, riscv64, loong64),
  /// use unofficial-builds.nodejs.org instead of the default mirror.
  pub unofficial_builds: Option<bool>,
  /// Overrides the unofficial-builds URL.
  pub unofficial_url: Option<String>,
}

/// One `[[auth]]` entry: a header to send to every URL under `prefix`.
///
/// The value comes from exactly one of `value`, `value_env`, `command`, or
/// `username` with `password`/`password_env` (sent as HTTP Basic).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthEntry {
  /// An `https` URL prefix, such as `https://mirror.corp/nodejs/`.
  pub prefix: String,
  /// The header name; `Authorization` by default.
  pub header: Option<String>,
  /// Put in front of the value with a space, such as `Bearer`.
  pub scheme: Option<String>,
  /// The value itself. Prefer `value_env` or `command`, which keep the
  /// secret out of the file.
  pub value: Option<String>,
  /// An environment variable that holds the value.
  pub value_env: Option<String>,
  /// A program and its arguments that print the value on stdout, such as
  /// `["aws", "codeartifact", "get-authorization-token", …]`. It runs at
  /// most once per rnm command, and only when a request needs it.
  pub command: Option<Vec<String>>,
  pub username: Option<String>,
  pub password: Option<String>,
  pub password_env: Option<String>,
}

/// `[http]`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpSettings {
  /// Read credentials from netrc. On by default.
  pub netrc: Option<bool>,
  /// The netrc file; `~/.netrc` by default (`%USERPROFILE%\_netrc` on
  /// Windows).
  pub netrc_file: Option<PathBuf>,
  /// PEM certificates to trust instead of the system's certificate store.
  pub ca_file: Option<PathBuf>,
  /// Send credentials over plain `http` to hosts other than localhost.
  pub allow_insecure_http_auth: Option<bool>,
}

/// `[version_files]`: which files name a project's version.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionFileSettings {
  /// Also read `devEngines.runtime` and `engines.node` from
  /// `package.json`, after `.nvmrc` and `.node-version` in each directory.
  pub package_json: Option<bool>,
}

/// The settings file exists but cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
  pub path: PathBuf,
  pub message: String,
}

impl fmt::Display for ConfigError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}: {}", self.path.display(), self.message)
  }
}

impl std::error::Error for ConfigError {}

impl Config {
  /// Reads the settings file found with `env`, or the defaults when there
  /// is none.
  ///
  /// # Errors
  ///
  /// Fails when the file exists but cannot be read or parsed, or when
  /// `RNM_CONFIG` names a file that does not exist.
  pub fn load(env: impl Fn(&str) -> Option<OsString>) -> Result<Self, ConfigError> {
    let explicit = env(CONFIG_ENV)
      .filter(|value| !value.is_empty())
      .map(PathBuf::from);
    let Some(path) = explicit.clone().or_else(|| default_path(&env)) else {
      return Ok(Self::default());
    };
    match std::fs::read_to_string(&path) {
      | Ok(text) => Self::parse(&text).map(|config| config.read_from(path.clone())),
      | Err(e) if e.kind() == std::io::ErrorKind::NotFound && explicit.is_none() => {
        return Ok(Self::default());
      },
      | Err(e) => Err(e.to_string()),
    }
    .map_err(|message| ConfigError { path, message })
  }

  /// Parses settings text.
  ///
  /// # Errors
  ///
  /// Returns the parser's message, which names the line and the problem.
  pub fn parse(text: &str) -> Result<Self, String> {
    toml::from_str(text).map_err(|e| e.to_string().trim_end().to_owned())
  }

  fn read_from(mut self, path: PathBuf) -> Self {
    self.path = Some(path);
    self
  }

  /// The settings file can be read by other users (unix only).
  #[must_use]
  pub fn is_readable_by_others(&self) -> bool {
    self
      .path
      .as_deref()
      .is_some_and(crate::fs::is_readable_by_others)
  }

  /// The file name for messages: the path when there is a file, otherwise
  /// `config.toml`.
  #[must_use]
  pub fn display_name(&self) -> String {
    self
      .path
      .as_deref()
      .map_or_else(|| FILE_NAME.to_owned(), |path| path.display().to_string())
  }
}

/// The default settings file location for this platform.
#[must_use]
pub fn default_path(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
  let var = |key: &str| {
    env(key)
      .filter(|value| !value.is_empty())
      .map(PathBuf::from)
  };
  let dir = if cfg!(windows) {
    var("APPDATA")
  } else {
    var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|home| home.join(".config")))
  }?;
  Some(dir.join(DIR_NAME).join(FILE_NAME))
}

/// Reads a boolean environment variable: `1`, `true`, `yes`, `on` or `0`,
/// `false`, `no`, `off`, in any case. `None` when unset, empty, or anything
/// else.
#[must_use]
pub fn env_flag(env: impl Fn(&str) -> Option<OsString>, names: &[&str]) -> Option<bool> {
  names.iter().find_map(|name| {
    let value = env(name)?.to_string_lossy().trim().to_ascii_lowercase();
    match value.as_str() {
      | "1" | "true" | "yes" | "on" => Some(true),
      | "0" | "false" | "no" | "off" => Some(false),
      | _ => None,
    }
  })
}

/// `path` with a leading `~` replaced by `home`.
#[must_use]
pub fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
  match (path.strip_prefix("~"), home) {
    | (Ok(rest), Some(home)) => home.join(rest),
    | _ => path.to_path_buf(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;

  fn reader(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let vars: HashMap<String, OsString> = vars
      .iter()
      .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
      .collect();
    move |key| vars.get(key).cloned()
  }

  #[test]
  fn parses_every_section() {
    let config = Config::parse(
      r#"
      [mirror]
      url = "https://mirror.corp/dist"
      fallback = ["https://nodejs.org/dist"]
      unofficial_builds = true

      [[auth]]
      prefix = "https://mirror.corp/"
      value_env = "TOKEN"
      scheme = "Bearer"

      [[auth]]
      prefix = "https://gitlab.corp/"
      header = "PRIVATE-TOKEN"
      command = ["glab", "auth", "token"]

      [http]
      netrc = false
      ca_file = "/etc/ca.pem"

      [version_files]
      package_json = true
      "#,
    )
    .unwrap();
    assert_eq!(
      config.mirror.url.as_deref(),
      Some("https://mirror.corp/dist")
    );
    assert_eq!(config.mirror.fallback, ["https://nodejs.org/dist"]);
    assert_eq!(config.mirror.unofficial_builds, Some(true));
    assert_eq!(config.auth.len(), 2);
    assert_eq!(config.auth[1].header.as_deref(), Some("PRIVATE-TOKEN"));
    assert_eq!(config.http.netrc, Some(false));
    assert_eq!(config.version_files.package_json, Some(true));
  }

  #[test]
  fn rejects_unknown_keys_with_the_line() {
    let error = Config::parse("[mirror]\nurl = \"x\"\nfallbacks = []\n").unwrap_err();
    assert!(error.contains("fallbacks"), "{error}");
    assert!(error.contains("line 3"), "{error}");
  }

  #[cfg(unix)]
  #[test]
  fn locates_the_file() {
    assert_eq!(
      default_path(reader(&[("HOME", "/home/u")])),
      Some(PathBuf::from("/home/u/.config/rnm/config.toml"))
    );
    assert_eq!(
      default_path(reader(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x")])),
      Some(PathBuf::from("/x/rnm/config.toml"))
    );
    assert_eq!(default_path(reader(&[])), None);
  }

  #[test]
  fn missing_default_file_is_empty_but_missing_explicit_file_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_str().unwrap();
    let config = Config::load(reader(&[
      ("HOME", home),
      ("XDG_CONFIG_HOME", home),
      ("APPDATA", home),
    ]))
    .unwrap();
    assert!(config.path.is_none());

    let missing = tmp.path().join("nope.toml");
    let error = Config::load(reader(&[(CONFIG_ENV, missing.to_str().unwrap())])).unwrap_err();
    assert_eq!(error.path, missing);
  }

  #[test]
  fn loads_the_explicit_file() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("c.toml");
    std::fs::write(&file, "[version_files]\npackage_json = true\n").unwrap();
    let config = Config::load(reader(&[(CONFIG_ENV, file.to_str().unwrap())])).unwrap();
    assert_eq!(config.version_files.package_json, Some(true));
    assert_eq!(config.path, Some(file.clone()));

    std::fs::write(&file, "[mirror\n").unwrap();
    let error = Config::load(reader(&[(CONFIG_ENV, file.to_str().unwrap())])).unwrap_err();
    assert!(error.to_string().starts_with(&file.display().to_string()));
  }

  #[test]
  fn reads_flags() {
    let env = reader(&[("A", "Yes"), ("B", "0"), ("C", "maybe")]);
    assert_eq!(env_flag(&env, &["A"]), Some(true));
    assert_eq!(env_flag(&env, &["B"]), Some(false));
    assert_eq!(env_flag(&env, &["C"]), None);
    assert_eq!(env_flag(&env, &["X", "B"]), Some(false));
  }

  #[test]
  fn expands_home() {
    assert_eq!(
      expand_home(Path::new("~/.netrc"), Some(Path::new("/h"))),
      Path::new("/h").join(".netrc")
    );
    assert_eq!(
      expand_home(Path::new("/a"), Some(Path::new("/h"))),
      Path::new("/a")
    );
  }
}
