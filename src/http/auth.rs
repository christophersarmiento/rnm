//! Credentials that are only sent where they belong.

use std::fmt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use ureq::http::{HeaderName, HeaderValue};

use super::url::{Origin, Url};

/// A header value that never appears in logs, errors, or `Debug` output.
#[derive(Clone)]
pub struct Secret(HeaderValue);

impl Secret {
  /// Wraps a header value.
  ///
  /// # Errors
  ///
  /// Returns [`InvalidSecret`] if the value is empty or contains characters
  /// that cannot appear in an HTTP header (CR, LF, NUL, and other control
  /// characters). Rejecting them prevents header injection; nvm silently
  /// deletes them instead, which hides typos.
  pub fn new(value: &str) -> Result<Self, InvalidSecret> {
    if value.trim().is_empty() {
      return Err(InvalidSecret);
    }
    let mut header = HeaderValue::from_str(value).map_err(|_| InvalidSecret)?;
    header.set_sensitive(true);
    Ok(Self(header))
  }

  pub(super) fn header_value(&self) -> &HeaderValue {
    &self.0
  }
}

impl fmt::Debug for Secret {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("Secret(<redacted>)")
  }
}

/// A credential value is empty or not a valid HTTP header value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSecret;

impl fmt::Display for InvalidSecret {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("the value is empty or contains characters that are not allowed in an HTTP header")
  }
}

impl std::error::Error for InvalidSecret {}

/// Where a credential may be sent.
///
/// A prefix scope, `https://mirror.corp/nodejs/`, covers
/// `https://mirror.corp/nodejs/v20/…` but not `https://mirror.corp/other/…`,
/// `http://mirror.corp/nodejs/…` (a downgrade), or any other host or port.
///
/// A host scope, for netrc entries, covers every https URL on one host (and
/// any URL on localhost), on any port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope(ScopeKind);

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopeKind {
  Prefix {
    origin: Origin,
    /// Always ends with `/`, so `/nodejs/` does not match `/nodejs-old/`.
    path_prefix: String,
  },
  Host {
    host: String,
    allow_plain_http: bool,
  },
}

impl Scope {
  /// The scope of everything under `base`.
  #[must_use]
  pub fn under(base: &Url) -> Self {
    let path = base.path();
    let path_prefix = if path.ends_with('/') {
      path.to_owned()
    } else {
      format!("{path}/")
    };
    Self(ScopeKind::Prefix {
      origin: base.origin(),
      path_prefix,
    })
  }

  /// Every https URL on `host`; plain http too when `allow_plain_http` or
  /// when the host is local.
  #[must_use]
  pub fn host(host: &str, allow_plain_http: bool) -> Self {
    Self(ScopeKind::Host {
      host: host.to_ascii_lowercase(),
      allow_plain_http,
    })
  }

  #[must_use]
  pub fn covers(&self, url: &Url) -> bool {
    match &self.0 {
      | ScopeKind::Prefix {
        origin,
        path_prefix,
      } => {
        let same_origin = url.origin() == *origin;
        let path = url.path();
        let is_base_itself = path == path_prefix.trim_end_matches('/');
        same_origin && (path.starts_with(path_prefix.as_str()) || is_base_itself)
      },
      | ScopeKind::Host {
        host,
        allow_plain_http,
      } => {
        let secure_enough = url.is_https() || url.is_loopback() || *allow_plain_http;
        url.origin().host == *host && secure_enough
      },
    }
  }

  /// How specific the scope is: when several cover a URL, the most specific
  /// wins, so a longer prefix beats a shorter one and any prefix beats a
  /// host.
  #[must_use]
  pub fn specificity(&self) -> usize {
    match &self.0 {
      | ScopeKind::Prefix { path_prefix, .. } => path_prefix.len() + 1,
      | ScopeKind::Host { .. } => 0,
    }
  }
}

/// Where a credential's value comes from.
pub enum CredentialValue {
  /// Known up front.
  Fixed(Secret),
  /// Printed by a helper program, which runs at most once, on first use.
  Command(HelperCommand),
}

impl fmt::Debug for CredentialValue {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::Fixed(secret) => secret.fmt(f),
      | Self::Command(command) => write!(f, "Command({:?})", command.program()),
    }
  }
}

/// A credential helper: a program that prints a token, like git's
/// `credential.helper` or pnpm's `tokenHelper`. Short-lived tokens (AWS
/// `CodeArtifact`, OIDC) need one.
pub struct HelperCommand {
  argv: Vec<String>,
  /// Put in front of the printed token with a space (`Bearer`).
  scheme: Option<String>,
  result: OnceLock<Result<Secret, String>>,
}

impl HelperCommand {
  /// A helper that runs `argv` (program first). Returns `None` when `argv`
  /// is empty.
  #[must_use]
  pub fn new(argv: Vec<String>, scheme: Option<String>) -> Option<Self> {
    (!argv.is_empty()).then(|| Self {
      argv,
      scheme,
      result: OnceLock::new(),
    })
  }

  /// The program's name, for messages.
  #[must_use]
  pub fn program(&self) -> &str {
    self.argv.first().map_or("", String::as_str)
  }

  /// Runs the helper once and returns its token, trimmed, with the scheme.
  ///
  /// # Errors
  ///
  /// A message when the program cannot start, fails, or prints nothing
  /// usable. It never contains the program's stdout.
  pub fn secret(&self) -> Result<&Secret, String> {
    self
      .result
      .get_or_init(|| self.run())
      .as_ref()
      .map_err(Clone::clone)
  }

  fn run(&self) -> Result<Secret, String> {
    let program = self.program();
    let output = Command::new(program)
      .args(&self.argv[1..])
      .stdin(Stdio::null())
      .stderr(Stdio::piped())
      .output()
      .map_err(|e| format!("cannot run '{program}': {e}"))?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      return Err(format!(
        "'{program}' failed ({}): {}",
        output.status,
        stderr.trim()
      ));
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let value = match &self.scheme {
      | Some(scheme) => format!("{scheme} {token}"),
      | None => token.clone(),
    };
    if token.is_empty() {
      return Err(format!("'{program}' printed no token"));
    }
    Secret::new(&value).map_err(|e| format!("'{program}' printed an unusable token: {e}"))
  }
}

/// A header to add to requests inside a scope.
#[derive(Debug)]
pub struct Credential {
  pub scope: Scope,
  pub header: HeaderName,
  pub value: CredentialValue,
  /// Where the credential came from, for error messages (for example
  /// `NVM_AUTH_HEADER`). Never the value itself.
  pub source: String,
}

impl Credential {
  /// A credential with a value known up front.
  #[must_use]
  pub fn fixed(scope: Scope, header: HeaderName, value: Secret, source: impl Into<String>) -> Self {
    Self {
      scope,
      header,
      value: CredentialValue::Fixed(value),
      source: source.into(),
    }
  }

  /// The header value, running a helper command if needed.
  ///
  /// # Errors
  ///
  /// See [`HelperCommand::secret`].
  pub fn secret(&self) -> Result<&Secret, String> {
    match &self.value {
      | CredentialValue::Fixed(secret) => Ok(secret),
      | CredentialValue::Command(command) => command.secret(),
    }
  }
}

/// `Basic <base64(user:password)>`, RFC 7617.
///
/// # Errors
///
/// See [`Secret::new`].
pub fn basic_auth(user: &str, password: &str) -> Result<Secret, InvalidSecret> {
  use base64::Engine as _;
  let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
  Secret::new(&format!("Basic {encoded}"))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
  }

  #[test]
  fn scope_is_origin_and_path_prefix() {
    let scope = Scope::under(&url("https://mirror.corp/nodejs"));
    assert!(scope.covers(&url("https://mirror.corp/nodejs")));
    assert!(scope.covers(&url("https://mirror.corp/nodejs/v20.0.0/node.tar.xz")));
    assert!(scope.covers(&url("https://MIRROR.corp:443/nodejs/index.json")));
    assert!(!scope.covers(&url("https://mirror.corp/nodejs-old/index.json")));
    assert!(!scope.covers(&url("https://mirror.corp/other")));
    assert!(!scope.covers(&url("http://mirror.corp/nodejs/index.json")));
    assert!(!scope.covers(&url("https://mirror.corp:8443/nodejs/index.json")));
    assert!(!scope.covers(&url("https://cdn.example/nodejs/index.json")));
  }

  #[test]
  fn root_scope_covers_whole_origin() {
    let scope = Scope::under(&url("https://mirror.corp"));
    assert!(scope.covers(&url("https://mirror.corp/anything")));
  }

  #[test]
  fn host_scope_needs_https_except_locally() {
    let scope = Scope::host("Mirror.Corp", false);
    assert!(scope.covers(&url("https://mirror.corp:8443/any")));
    assert!(!scope.covers(&url("http://mirror.corp/any")));
    assert!(!scope.covers(&url("https://other.corp/any")));
    assert!(Scope::host("localhost", false).covers(&url("http://localhost:9/x")));
    assert!(Scope::host("mirror.corp", true).covers(&url("http://mirror.corp/x")));
  }

  #[test]
  fn longer_prefixes_are_more_specific() {
    let short = Scope::under(&url("https://h/a"));
    let long = Scope::under(&url("https://h/a/b"));
    assert!(long.specificity() > short.specificity());
    assert!(short.specificity() > Scope::host("h", false).specificity());
  }

  #[test]
  fn encodes_basic_auth() {
    let secret = basic_auth("user", "p@ss:word").unwrap();
    assert_eq!(
      secret.header_value().to_str().unwrap(),
      "Basic dXNlcjpwQHNzOndvcmQ="
    );
  }

  #[cfg(unix)]
  #[test]
  fn runs_helpers_once() {
    let tmp = tempfile::tempdir().unwrap();
    let counter = tmp.path().join("runs");
    let script = format!("echo x >> '{}'; echo ' tok '", counter.display());
    let helper = HelperCommand::new(
      vec!["sh".into(), "-c".into(), script],
      Some("Bearer".into()),
    )
    .unwrap();
    assert_eq!(
      helper.secret().unwrap().header_value().to_str().unwrap(),
      "Bearer tok"
    );
    helper.secret().unwrap();
    assert_eq!(
      std::fs::read_to_string(&counter).unwrap().lines().count(),
      1
    );

    let failing = HelperCommand::new(
      vec![
        "sh".into(),
        "-c".into(),
        "echo secret; echo oops >&2; exit 3".into(),
      ],
      None,
    )
    .unwrap();
    let error = failing.secret().unwrap_err();
    assert!(error.contains("oops"), "{error}");
    assert!(!error.contains("secret"), "{error}");
    let empty = HelperCommand::new(vec!["true".into()], None).unwrap();
    assert!(empty.secret().unwrap_err().contains("no token"));
  }

  #[test]
  fn secrets_reject_injection_and_hide_in_debug() {
    assert_eq!(
      Secret::new("Bearer a\r\nX-Evil: 1").unwrap_err(),
      InvalidSecret
    );
    assert_eq!(Secret::new("a\0b").unwrap_err(), InvalidSecret);
    assert_eq!(Secret::new("  ").unwrap_err(), InvalidSecret);
    let secret = Secret::new("Bearer hunter2").unwrap();
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
    assert!(!format!("{:?}", secret.header_value()).contains("hunter2"));
  }
}
