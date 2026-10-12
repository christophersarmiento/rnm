//! Credentials that are only sent where they belong.

use std::fmt;

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

/// Where a credential may be sent: one origin and a path prefix under it.
///
/// `https://mirror.corp/nodejs/` covers `https://mirror.corp/nodejs/v20/…`
/// but not `https://mirror.corp/other/…`, `http://mirror.corp/nodejs/…`
/// (a downgrade), or any other host or port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
  origin: Origin,
  /// Always ends with `/`, so `/nodejs/` does not match `/nodejs-old/`.
  path_prefix: String,
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
    Self {
      origin: base.origin(),
      path_prefix,
    }
  }

  #[must_use]
  pub fn covers(&self, url: &Url) -> bool {
    let same_origin = url.origin() == self.origin;
    let path = url.path();
    let is_base_itself = path == self.path_prefix.trim_end_matches('/');
    same_origin && (path.starts_with(&self.path_prefix) || is_base_itself)
  }
}

/// A header to add to requests inside a scope.
#[derive(Clone, Debug)]
pub struct Credential {
  pub scope: Scope,
  pub header: HeaderName,
  pub value: Secret,
  /// Where the credential came from, for error messages (for example
  /// `NVM_AUTH_HEADER`). Never the value itself.
  pub source: String,
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
