//! The Node.js distribution mirrors and the credentials for them, from
//! `config.toml` and from environment variables that rnm shares with nvm
//! and fnm (environment variables win).

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use ureq::http::HeaderName;
use ureq::http::header::AUTHORIZATION;
use ureq::tls::{Certificate, PemItem, parse_pem};

use super::Client;
use super::auth::{Credential, CredentialValue, HelperCommand, Scope, Secret, basic_auth};
use super::netrc::parse_netrc;
use super::url::{InvalidUrl, Url};
use crate::config::{AuthEntry, Config, env_flag, expand_home};

/// The official distribution site.
pub const DEFAULT_NODE_MIRROR: &str = "https://nodejs.org/dist";

/// Builds for systems nodejs.org does not cover (musl, riscv64, loong64).
pub const UNOFFICIAL_BUILDS_MIRROR: &str = "https://unofficial-builds.nodejs.org/download/release";

/// Names the built-in default in messages, where a variable name would
/// otherwise go.
const DEFAULT_MIRROR_SOURCE: &str = "the default mirror";
const UNOFFICIAL_MIRROR_SOURCE: &str = "unofficial builds";

/// Mirror variables, highest precedence first.
const MIRROR_VARS: [&str; 3] = [
  "RNM_NODE_MIRROR",
  "NVM_NODEJS_ORG_MIRROR",
  "FNM_NODE_DIST_MIRROR",
];

/// Extra mirrors, separated by commas or whitespace.
const FALLBACK_VAR: &str = "RNM_NODE_MIRROR_FALLBACK";

const UNOFFICIAL_VARS: [&str; 1] = ["RNM_UNOFFICIAL_BUILDS"];
const NETRC_SWITCH_VARS: [&str; 1] = ["RNM_NETRC"];
/// The netrc file location, as Python and several other tools read it.
const NETRC_FILE_VAR: &str = "NETRC";
const CA_FILE_VAR: &str = "RNM_CA_FILE";

/// Variables holding a full `Authorization` header value (for example
/// `Bearer abc` or `Basic dXNlcjpwYXNz`), highest precedence first.
const AUTH_HEADER_VARS: [&str; 3] = ["RNM_AUTH_HEADER", "NVM_AUTH_HEADER", "FNM_AUTH_HEADER"];

/// A mirror's base URL and where it was configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mirror {
  pub url: Url,
  /// A variable name, a config key, or [`DEFAULT_MIRROR_SOURCE`].
  pub source: String,
}

/// The mirrors to use, in order, and a client that holds every
/// credential.
pub struct Network {
  /// Never empty; the first is the main mirror.
  pub mirrors: Vec<Mirror>,
  pub client: Client,
  /// Problems that did not stop rnm, such as a netrc file other users can
  /// read.
  pub warnings: Vec<String>,
  /// Where each credential came from, for `rnm doctor`. Never the values.
  pub credential_sources: Vec<String>,
}

/// Why the network settings are unusable. Messages never contain secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorError {
  /// A mirror is not an `http` or `https` URL.
  InvalidUrl { source: String, error: InvalidUrl },
  /// A credential's value cannot be sent as a header.
  InvalidAuthHeader { source: String },
  /// Credentials would travel unencrypted to a non-local host.
  AuthOverPlainHttp { source: String, url: String },
  /// An `[[auth]]` entry is incomplete or contradictory.
  InvalidAuthEntry { source: String, reason: String },
  /// The certificate file cannot be used.
  CaFile { path: PathBuf, reason: String },
}

impl fmt::Display for MirrorError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::InvalidUrl { source, error } => write!(f, "{source}: {error}"),
      | Self::InvalidAuthHeader { source } => write!(
        f,
        "{source} is empty or contains characters that are not allowed in an HTTP header \
         (such as a line break)"
      ),
      | Self::AuthOverPlainHttp { source, url } => write!(
        f,
        "{source} applies to {url}, which uses plain http; rnm only sends credentials over \
         https (or to localhost). To allow it anyway, set allow_insecure_http_auth = true in \
         the [http] section of config.toml"
      ),
      | Self::InvalidAuthEntry { source, reason } => write!(f, "{source}: {reason}"),
      | Self::CaFile { path, reason } => write!(f, "{}: {reason}", path.display()),
    }
  }
}

impl std::error::Error for MirrorError {}

/// Reads settings from the environment, with `config.toml` underneath.
struct Settings<'a, E> {
  config: &'a Config,
  env: E,
  allow_insecure: bool,
}

impl Network {
  /// Builds the mirror list and client from environment variables only, as
  /// if there were no `config.toml`.
  ///
  /// # Errors
  ///
  /// See [`MirrorError`].
  pub fn from_env(env: impl Fn(&str) -> Option<OsString>) -> Result<Self, MirrorError> {
    Self::from_settings(&Config::default(), env, false)
  }

  /// Builds the mirror list and client.
  ///
  /// - Main mirror: `RNM_NODE_MIRROR`, `NVM_NODEJS_ORG_MIRROR`,
  ///   `FNM_NODE_DIST_MIRROR`, `mirror.url`; else unofficial-builds when
  ///   `needs_unofficial_builds` and `RNM_UNOFFICIAL_BUILDS` or
  ///   `mirror.unofficial_builds` allow it; else [`DEFAULT_NODE_MIRROR`].
  /// - Fallbacks: `RNM_NODE_MIRROR_FALLBACK`, else `mirror.fallback`.
  /// - Credentials, first match wins: `RNM_AUTH_HEADER`, `NVM_AUTH_HEADER`,
  ///   `FNM_AUTH_HEADER` (sent as `Authorization` to the main mirror only);
  ///   `user:password@` in a mirror URL; `[[auth]]` entries, the longest
  ///   matching prefix first; netrc entries by host.
  /// - Certificates: `RNM_CA_FILE`, else `http.ca_file`, else the system's
  ///   certificate store.
  ///
  /// Empty variables count as unset.
  ///
  /// # Errors
  ///
  /// See [`MirrorError`].
  pub fn from_settings(
    config: &Config,
    env: impl Fn(&str) -> Option<OsString>,
    needs_unofficial_builds: bool,
  ) -> Result<Self, MirrorError> {
    let settings = Settings {
      config,
      allow_insecure: config.http.allow_insecure_http_auth == Some(true),
      env,
    };
    let mut warnings = Vec::new();
    let mut credentials = Vec::new();
    let mut mirrors = Vec::new();
    let main = settings.main_mirror(needs_unofficial_builds);
    for (index, (source, text)) in std::iter::once(main)
      .chain(settings.fallbacks())
      .enumerate()
    {
      let url = Url::parse(&text).map_err(|error| MirrorError::InvalidUrl {
        source: source.clone(),
        error,
      })?;
      let (url, userinfo) = url.split_userinfo();
      if let Some(userinfo) = userinfo {
        let value = basic_auth(&userinfo.user, &userinfo.password).map_err(|_| {
          MirrorError::InvalidAuthHeader {
            source: format!("the user name and password in {source}"),
          }
        })?;
        let label = format!("the user name and password in {source}");
        settings.check_transport(&label, &url)?;
        credentials.push(Credential::fixed(
          Scope::under(&url),
          AUTHORIZATION,
          value,
          label,
        ));
      }
      let is_main = index == 0;
      if is_main && let Some(credential) = settings.env_credential(&url)? {
        credentials.insert(0, credential);
      }
      mirrors.push(Mirror { url, source });
    }
    credentials.extend(settings.auth_entries(&mut warnings)?);
    credentials.extend(settings.netrc(&mut warnings));
    let credential_sources = credentials.iter().map(|c| c.source.clone()).collect();
    let client = match settings.ca_certificates()? {
      | Some(certificates) => Client::with_ca_certificates(credentials, certificates),
      | None => Client::new(credentials),
    };
    Ok(Self {
      mirrors,
      client,
      warnings,
      credential_sources,
    })
  }

  /// The main mirror.
  #[must_use]
  pub fn mirror(&self) -> &Mirror {
    &self.mirrors[0]
  }
}

impl<E: Fn(&str) -> Option<OsString>> Settings<'_, E> {
  fn var(&self, key: &str) -> Option<String> {
    let value = (self.env)(key)?.to_string_lossy().trim().to_owned();
    (!value.is_empty()).then_some(value)
  }

  fn first_var(&self, vars: &[&'static str]) -> Option<(String, String)> {
    vars
      .iter()
      .find_map(|var| Some(((*var).to_owned(), self.var(var)?)))
  }

  fn config_key(&self, key: &str) -> String {
    format!("{} ({key})", self.config.display_name())
  }

  fn main_mirror(&self, needs_unofficial_builds: bool) -> (String, String) {
    if let Some(found) = self.first_var(&MIRROR_VARS) {
      return found;
    }
    if let Some(url) = &self.config.mirror.url {
      return (self.config_key("mirror.url"), url.clone());
    }
    let unofficial_allowed = env_flag(&self.env, &UNOFFICIAL_VARS)
      .or(self.config.mirror.unofficial_builds)
      .unwrap_or(false);
    if needs_unofficial_builds && unofficial_allowed {
      let url = self.config.mirror.unofficial_url.as_deref();
      return (
        UNOFFICIAL_MIRROR_SOURCE.to_owned(),
        url.unwrap_or(UNOFFICIAL_BUILDS_MIRROR).to_owned(),
      );
    }
    (
      DEFAULT_MIRROR_SOURCE.to_owned(),
      DEFAULT_NODE_MIRROR.to_owned(),
    )
  }

  fn fallbacks(&self) -> Vec<(String, String)> {
    if let Some(list) = self.var(FALLBACK_VAR) {
      return list
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|url| !url.is_empty())
        .map(|url| (FALLBACK_VAR.to_owned(), url.to_owned()))
        .collect();
    }
    let source = self.config_key("mirror.fallback");
    self
      .config
      .mirror
      .fallback
      .iter()
      .map(|url| (source.clone(), url.clone()))
      .collect()
  }

  /// Refuses to send a credential in clear text, unless the user allowed
  /// it.
  fn check_transport(&self, source: &str, url: &Url) -> Result<(), MirrorError> {
    let sends_in_clear = !url.is_https() && !url.is_loopback();
    if sends_in_clear && !self.allow_insecure {
      return Err(MirrorError::AuthOverPlainHttp {
        source: source.to_owned(),
        url: url.to_string(),
      });
    }
    Ok(())
  }

  /// nvm's and fnm's `*_AUTH_HEADER`, scoped to the main mirror.
  fn env_credential(&self, mirror: &Url) -> Result<Option<Credential>, MirrorError> {
    let Some((var, value)) = self.first_var(&AUTH_HEADER_VARS) else {
      return Ok(None);
    };
    self.check_transport(&var, mirror)?;
    let value = Secret::new(&value).map_err(|_| MirrorError::InvalidAuthHeader {
      source: var.clone(),
    })?;
    Ok(Some(Credential::fixed(
      Scope::under(mirror),
      AUTHORIZATION,
      value,
      var,
    )))
  }

  fn auth_entries(&self, warnings: &mut Vec<String>) -> Result<Vec<Credential>, MirrorError> {
    let mut credentials = self
      .config
      .auth
      .iter()
      .enumerate()
      .map(|(index, entry)| self.auth_entry(index, entry, warnings))
      .collect::<Result<Vec<_>, _>>()?;
    credentials.sort_by_key(|credential| std::cmp::Reverse(credential.scope.specificity()));
    Ok(credentials)
  }

  fn auth_entry(
    &self,
    index: usize,
    entry: &AuthEntry,
    warnings: &mut Vec<String>,
  ) -> Result<Credential, MirrorError> {
    let source = format!(
      "{} ([[auth]] #{}, {})",
      self.config.display_name(),
      index + 1,
      entry.prefix
    );
    let invalid = |reason: &str| MirrorError::InvalidAuthEntry {
      source: source.clone(),
      reason: reason.to_owned(),
    };
    let prefix = Url::parse(&entry.prefix).map_err(|error| MirrorError::InvalidUrl {
      source: source.clone(),
      error,
    })?;
    self.check_transport(&source, &prefix)?;
    let header = match &entry.header {
      | Some(name) => HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| invalid(&format!("'{name}' is not a valid header name")))?,
      | None => AUTHORIZATION,
    };

    let sources_given = [
      entry.value.is_some(),
      entry.value_env.is_some(),
      entry.command.is_some(),
      entry.username.is_some(),
    ]
    .into_iter()
    .filter(|given| *given)
    .count();
    if sources_given != 1 {
      return Err(invalid(
        "set exactly one of value, value_env, command, or username (with password or \
         password_env)",
      ));
    }
    let with_scheme = |token: &str| match &entry.scheme {
      | Some(scheme) => format!("{scheme} {token}"),
      | None => token.to_owned(),
    };
    let fixed = |text: &str| {
      Secret::new(text)
        .map(CredentialValue::Fixed)
        .map_err(|_| MirrorError::InvalidAuthHeader {
          source: source.clone(),
        })
    };
    let literal_used = entry.value.is_some() || entry.password.is_some();
    if literal_used && self.config.is_readable_by_others() {
      warnings.push(format!(
        "warning: {} holds a credential and other users can read it; run chmod 600 on it, or \
         use value_env or command instead",
        self.config.display_name()
      ));
    }

    let value = if let Some(value) = &entry.value {
      fixed(&with_scheme(value))?
    } else if let Some(var) = &entry.value_env {
      let value = self
        .var(var)
        .ok_or_else(|| invalid(&format!("the environment variable {var} is not set")))?;
      fixed(&with_scheme(&value))?
    } else if let Some(argv) = &entry.command {
      let helper = HelperCommand::new(argv.clone(), entry.scheme.clone())
        .ok_or_else(|| invalid("command is empty"))?;
      CredentialValue::Command(helper)
    } else {
      let user = entry.username.as_deref().unwrap_or_default();
      let password = match (&entry.password, &entry.password_env) {
        | (Some(password), None) => password.clone(),
        | (None, Some(var)) => self
          .var(var)
          .ok_or_else(|| invalid(&format!("the environment variable {var} is not set")))?,
        | _ => {
          return Err(invalid(
            "username needs exactly one of password or password_env",
          ));
        },
      };
      let secret = basic_auth(user, &password).map_err(|_| MirrorError::InvalidAuthHeader {
        source: source.clone(),
      })?;
      CredentialValue::Fixed(secret)
    };
    Ok(Credential {
      scope: Scope::under(&prefix),
      header,
      value,
      source,
    })
  }

  fn netrc(&self, warnings: &mut Vec<String>) -> Vec<Credential> {
    let enabled = env_flag(&self.env, &NETRC_SWITCH_VARS)
      .or(self.config.http.netrc)
      .unwrap_or(true);
    if !enabled {
      return Vec::new();
    }
    let Some(path) = self.netrc_path() else {
      return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
      return Vec::new();
    };
    let entries = parse_netrc(&text);
    if !entries.is_empty() && crate::fs::is_readable_by_others(&path) {
      warnings.push(format!(
        "warning: {} holds passwords and other users can read it; run chmod 600 {}",
        path.display(),
        path.display()
      ));
    }
    let source = path.display().to_string();
    entries
      .into_iter()
      .filter_map(|entry| {
        let value = basic_auth(&entry.login, &entry.password).ok()?;
        Some(Credential::fixed(
          Scope::host(&entry.machine, self.allow_insecure),
          AUTHORIZATION,
          value,
          format!("{source} (machine {})", entry.machine),
        ))
      })
      .collect()
  }

  fn home(&self) -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    self.var(key).map(PathBuf::from)
  }

  fn netrc_path(&self) -> Option<PathBuf> {
    let home = self.home();
    let configured = self
      .var(NETRC_FILE_VAR)
      .map(PathBuf::from)
      .or_else(|| self.config.http.netrc_file.clone());
    if let Some(path) = configured {
      return Some(expand_home(&path, home.as_deref()));
    }
    let home = home?;
    let names: &[&str] = if cfg!(windows) {
      &["_netrc", ".netrc"]
    } else {
      &[".netrc"]
    };
    names
      .iter()
      .map(|name| home.join(name))
      .find(|path| path.is_file())
  }

  fn ca_certificates(&self) -> Result<Option<Vec<Certificate<'static>>>, MirrorError> {
    let configured = self
      .var(CA_FILE_VAR)
      .map(PathBuf::from)
      .or_else(|| self.config.http.ca_file.clone());
    let Some(path) = configured else {
      return Ok(None);
    };
    let path = expand_home(&path, self.home().as_deref());
    read_certificates(&path).map(Some)
  }
}

/// Every certificate in a PEM file.
fn read_certificates(path: &Path) -> Result<Vec<Certificate<'static>>, MirrorError> {
  let failure = |reason: String| MirrorError::CaFile {
    path: path.to_path_buf(),
    reason,
  };
  let pem = std::fs::read(path).map_err(|e| failure(e.to_string()))?;
  let mut certificates = Vec::new();
  for item in parse_pem(&pem) {
    match item {
      | Ok(PemItem::Certificate(certificate)) => certificates.push(certificate.to_owned()),
      | Ok(_) => {},
      | Err(e) => return Err(failure(format!("not a valid PEM file: {e}"))),
    }
  }
  if certificates.is_empty() {
    return Err(failure("contains no PEM certificates".to_owned()));
  }
  Ok(certificates)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn network(config: &Config, vars: &[(&str, &str)]) -> Result<Network, MirrorError> {
    let vars: Vec<(String, OsString)> = vars
      .iter()
      .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
      .collect();
    Network::from_settings(
      config,
      |key| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
      false,
    )
  }

  fn env_only(vars: &[(&str, &str)]) -> Result<Network, MirrorError> {
    network(&Config::default(), vars)
  }

  fn config(text: &str) -> Config {
    Config::parse(text).unwrap()
  }

  fn urls(network: &Network) -> Vec<String> {
    network.mirrors.iter().map(|m| m.url.to_string()).collect()
  }

  #[test]
  fn defaults_to_nodejs_org_without_credentials() {
    let n = env_only(&[]).unwrap();
    assert_eq!(urls(&n), [DEFAULT_NODE_MIRROR]);
    assert_eq!(n.credential_sources, Vec::<String>::new());
  }

  #[test]
  fn mirror_precedence() {
    let file = config("[mirror]\nurl = \"https://c/d\"\n");
    assert_eq!(urls(&network(&file, &[]).unwrap()), ["https://c/d"]);
    let n = network(
      &file,
      &[
        ("FNM_NODE_DIST_MIRROR", "https://f/d"),
        ("NVM_NODEJS_ORG_MIRROR", "https://n/d"),
      ],
    )
    .unwrap();
    assert_eq!(urls(&n), ["https://n/d"]);
    let n = env_only(&[
      ("NVM_NODEJS_ORG_MIRROR", "https://n/d"),
      ("RNM_NODE_MIRROR", "https://r/d"),
    ])
    .unwrap();
    assert_eq!(urls(&n), ["https://r/d"]);
    let n = env_only(&[
      ("RNM_NODE_MIRROR", ""),
      ("NVM_NODEJS_ORG_MIRROR", "https://n/d"),
    ])
    .unwrap();
    assert_eq!(urls(&n), ["https://n/d"]);
  }

  #[test]
  fn fallbacks_follow_the_main_mirror() {
    let file = config("[mirror]\nfallback = [\"https://a/d\", \"https://b/d\"]\n");
    assert_eq!(
      urls(&network(&file, &[]).unwrap()),
      [DEFAULT_NODE_MIRROR, "https://a/d", "https://b/d"]
    );
    let n = network(&file, &[(FALLBACK_VAR, "https://x/d, https://y/d")]).unwrap();
    assert_eq!(
      urls(&n),
      [DEFAULT_NODE_MIRROR, "https://x/d", "https://y/d"]
    );
  }

  #[test]
  fn unofficial_builds_only_when_allowed_and_needed() {
    let vars = |config: &Config, needed: bool, env: &[(&str, &str)]| {
      let env: Vec<(String, OsString)> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
        .collect();
      let n = Network::from_settings(
        config,
        |key| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()),
        needed,
      )
      .unwrap();
      n.mirror().url.to_string()
    };
    let on = config("[mirror]\nunofficial_builds = true\n");
    assert_eq!(vars(&on, true, &[]), UNOFFICIAL_BUILDS_MIRROR);
    assert_eq!(vars(&on, false, &[]), DEFAULT_NODE_MIRROR);
    assert_eq!(vars(&Config::default(), true, &[]), DEFAULT_NODE_MIRROR);
    assert_eq!(
      vars(&Config::default(), true, &[("RNM_UNOFFICIAL_BUILDS", "1")]),
      UNOFFICIAL_BUILDS_MIRROR
    );
    assert_eq!(
      vars(&on, true, &[("RNM_UNOFFICIAL_BUILDS", "false")]),
      DEFAULT_NODE_MIRROR
    );
    assert_eq!(
      vars(&on, true, &[("RNM_NODE_MIRROR", "https://m/d")]),
      "https://m/d"
    );
  }

  #[test]
  fn env_credential_comes_first_and_is_scoped_to_the_main_mirror() {
    let file = config(
      "[mirror]\nfallback = [\"https://other/d\"]\n[[auth]]\nprefix = \"https://mirror.corp/\"\n\
       value = \"Bearer file\"\n",
    );
    let n = network(
      &file,
      &[
        ("NVM_NODEJS_ORG_MIRROR", "https://mirror.corp/nodejs"),
        ("NVM_AUTH_HEADER", "Bearer abc"),
      ],
    )
    .unwrap();
    assert_eq!(n.credential_sources[0], "NVM_AUTH_HEADER");
    assert!(n.credential_sources[1].contains("[[auth]] #1"));
  }

  #[test]
  fn userinfo_becomes_a_credential() {
    let n = env_only(&[("RNM_NODE_MIRROR", "https://u:p@mirror.corp/d")]).unwrap();
    assert_eq!(urls(&n), ["https://mirror.corp/d"]);
    assert_eq!(
      n.credential_sources,
      ["the user name and password in RNM_NODE_MIRROR"]
    );
  }

  #[test]
  fn validates_auth_entries() {
    let error = |text: &str, vars: &[(&str, &str)]| {
      network(&config(text), vars)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default()
    };
    let entry = |body: &str| format!("[[auth]]\nprefix = \"https://h/\"\n{body}\n");
    assert!(error(&entry(""), &[]).contains("exactly one of"));
    assert!(error(&entry("value = \"a\"\nvalue_env = \"B\""), &[]).contains("exactly one of"));
    assert!(error(&entry("value_env = \"MISSING\""), &[]).contains("MISSING is not set"));
    assert!(error(&entry("value = \"a\"\nheader = \"bad header\""), &[]).contains("header name"));
    assert!(error(&entry("username = \"u\""), &[]).contains("password"));
    assert!(error(&entry("command = []"), &[]).contains("command is empty"));
    assert!(error("[[auth]]\nprefix = \"http://h/\"\nvalue = \"a\"\n", &[]).contains("plain http"));
    assert!(
      network(
        &config("[http]\nallow_insecure_http_auth = true\n[[auth]]\nprefix = \"http://h/\"\nvalue = \"a\"\n"),
        &[]
      )
      .is_ok()
    );
    assert!(
      network(
        &config(&entry("value_env = \"T\"\nscheme = \"Bearer\"")),
        &[("T", "x")]
      )
      .is_ok()
    );
    assert!(
      network(
        &config(&entry("username = \"u\"\npassword_env = \"P\"")),
        &[("P", "x")]
      )
      .is_ok()
    );
  }

  #[test]
  fn longer_auth_prefixes_win() {
    let file = config(
      "[[auth]]\nprefix = \"https://h/\"\nvalue = \"a\"\n\
       [[auth]]\nprefix = \"https://h/deep/\"\nvalue = \"b\"\n",
    );
    let n = network(&file, &[]).unwrap();
    assert!(n.credential_sources[0].contains("#2"));
  }

  #[test]
  fn reads_netrc_unless_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let netrc = tmp.path().join("netrc");
    std::fs::write(&netrc, "machine mirror.corp login u password p\n").unwrap();
    let path = netrc.to_str().unwrap();
    let n = env_only(&[("NETRC", path)]).unwrap();
    assert_eq!(n.credential_sources.len(), 1);
    assert!(n.credential_sources[0].ends_with("(machine mirror.corp)"));
    let off = env_only(&[("NETRC", path), ("RNM_NETRC", "0")]).unwrap();
    assert_eq!(off.credential_sources, Vec::<String>::new());
    let off = network(&config("[http]\nnetrc = false\n"), &[("NETRC", path)]).unwrap();
    assert_eq!(off.credential_sources, Vec::<String>::new());
  }

  #[test]
  fn rejects_bad_settings() {
    assert!(matches!(
      env_only(&[("RNM_NODE_MIRROR", "mirror.corp/dist")]).err(),
      Some(MirrorError::InvalidUrl { source, .. }) if source == "RNM_NODE_MIRROR"
    ));
    assert_eq!(
      env_only(&[("RNM_AUTH_HEADER", "Bearer a\nX: y")]).err(),
      Some(MirrorError::InvalidAuthHeader {
        source: "RNM_AUTH_HEADER".to_owned()
      })
    );
    assert!(matches!(
      env_only(&[
        ("RNM_NODE_MIRROR", "http://mirror.corp/d"),
        ("FNM_AUTH_HEADER", "Bearer a")
      ]).err(),
      Some(MirrorError::AuthOverPlainHttp { source, .. }) if source == "FNM_AUTH_HEADER"
    ));
    assert!(
      env_only(&[
        ("RNM_NODE_MIRROR", "http://127.0.0.1:9/d"),
        ("RNM_AUTH_HEADER", "Bearer a")
      ])
      .is_ok()
    );
  }

  #[test]
  fn reads_ca_files() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("empty.pem");
    std::fs::write(&empty, "nothing here\n").unwrap();
    let error = env_only(&[(CA_FILE_VAR, empty.to_str().unwrap())])
      .err()
      .unwrap()
      .to_string();
    assert!(error.contains("no PEM certificates"), "{error}");
    let missing = tmp.path().join("missing.pem");
    assert!(matches!(
      env_only(&[(CA_FILE_VAR, missing.to_str().unwrap())]).err(),
      Some(MirrorError::CaFile { .. })
    ));
  }

  #[test]
  fn errors_never_contain_the_secret() {
    let error = env_only(&[
      ("RNM_NODE_MIRROR", "http://mirror.corp/d"),
      ("RNM_AUTH_HEADER", "Bearer hunter2"),
    ])
    .err()
    .unwrap();
    assert!(!error.to_string().contains("hunter2"));
    assert!(!format!("{error:?}").contains("hunter2"));
  }
}
