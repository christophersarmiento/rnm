//! The Node.js distribution mirror and its credentials, from environment
//! variables that rnm shares with nvm and fnm.

use std::ffi::OsString;
use std::fmt;

use ureq::http::header::AUTHORIZATION;

use super::auth::{Credential, Scope, Secret};
use super::url::{InvalidUrl, Url};

/// The official distribution site.
pub const DEFAULT_NODE_MIRROR: &str = "https://nodejs.org/dist";

/// Names the built-in default in error messages, where a variable name
/// would otherwise go.
const DEFAULT_MIRROR_SOURCE: &str = "the default mirror";

/// Mirror variables, highest precedence first.
const MIRROR_VARS: [&str; 3] = [
  "RNM_NODE_MIRROR",
  "NVM_NODEJS_ORG_MIRROR",
  "FNM_NODE_DIST_MIRROR",
];

/// Variables holding a full `Authorization` header value (for example
/// `Bearer abc` or `Basic dXNlcjpwYXNz`), highest precedence first.
const AUTH_HEADER_VARS: [&str; 3] = ["RNM_AUTH_HEADER", "NVM_AUTH_HEADER", "FNM_AUTH_HEADER"];

/// The mirror to download Node.js from, plus the credential for it.
#[derive(Clone, Debug)]
pub struct Mirror {
  pub url: Url,
  /// Sent only to URLs under [`Mirror::url`].
  pub credential: Option<Credential>,
}

/// Why the mirror settings are unusable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MirrorError {
  /// A mirror variable is not an `http` or `https` URL.
  InvalidUrl {
    var: &'static str,
    error: InvalidUrl,
  },
  /// An auth variable's value cannot be sent as a header.
  InvalidAuthHeader { var: &'static str },
  /// Credentials would travel unencrypted to a non-local host.
  AuthOverPlainHttp { var: &'static str, url: String },
}

impl fmt::Display for MirrorError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      | Self::InvalidUrl { var, error } => write!(f, "{var}: {error}"),
      | Self::InvalidAuthHeader { var } => write!(
        f,
        "{var} is empty or contains characters that are not allowed in an HTTP header \
         (such as a line break)"
      ),
      | Self::AuthOverPlainHttp { var, url } => write!(
        f,
        "{var} is set, but the mirror {url} uses plain http; rnm only sends credentials \
         over https (or to localhost)"
      ),
    }
  }
}

impl std::error::Error for MirrorError {}

impl Mirror {
  /// Reads the mirror and credential from the environment.
  ///
  /// - URL: `RNM_NODE_MIRROR`, `NVM_NODEJS_ORG_MIRROR`, `FNM_NODE_DIST_MIRROR`,
  ///   else [`DEFAULT_NODE_MIRROR`].
  /// - Credential: `RNM_AUTH_HEADER`, `NVM_AUTH_HEADER`, `FNM_AUTH_HEADER`,
  ///   sent as the `Authorization` header, as nvm and fnm do. It is scoped to
  ///   the mirror URL, so it is never sent to another host, port, scheme, or
  ///   path, even after a redirect.
  ///
  /// Empty variables count as unset.
  ///
  /// # Errors
  ///
  /// See [`MirrorError`].
  pub fn from_env(env: impl Fn(&str) -> Option<OsString>) -> Result<Self, MirrorError> {
    let first_set = |vars: [&'static str; 3]| {
      vars.into_iter().find_map(|var| {
        let value = env(var)?.to_string_lossy().trim().to_owned();
        (!value.is_empty()).then_some((var, value))
      })
    };

    let (url_var, url_text) =
      first_set(MIRROR_VARS).unwrap_or((DEFAULT_MIRROR_SOURCE, DEFAULT_NODE_MIRROR.to_owned()));
    let url = Url::parse(&url_text).map_err(|error| MirrorError::InvalidUrl {
      var: url_var,
      error,
    })?;

    let credential = match first_set(AUTH_HEADER_VARS) {
      | None => None,
      | Some((var, value)) => {
        let sends_in_clear = !url.is_https() && !url.is_loopback();
        if sends_in_clear {
          return Err(MirrorError::AuthOverPlainHttp {
            var,
            url: url.to_string(),
          });
        }
        let value = Secret::new(&value).map_err(|_| MirrorError::InvalidAuthHeader { var })?;
        Some(Credential {
          scope: Scope::under(&url),
          header: AUTHORIZATION,
          value,
          source: var.to_owned(),
        })
      },
    };

    Ok(Self { url, credential })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn mirror(vars: &[(&str, &str)]) -> Result<Mirror, MirrorError> {
    let vars: Vec<(String, OsString)> = vars
      .iter()
      .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
      .collect();
    Mirror::from_env(|key| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
  }

  #[test]
  fn defaults_to_nodejs_org_without_credentials() {
    let m = mirror(&[]).unwrap();
    assert_eq!(m.url.to_string(), DEFAULT_NODE_MIRROR);
    assert!(m.credential.is_none());
  }

  #[test]
  fn mirror_precedence() {
    let m = mirror(&[
      ("FNM_NODE_DIST_MIRROR", "https://f/d"),
      ("NVM_NODEJS_ORG_MIRROR", "https://n/d"),
    ])
    .unwrap();
    assert_eq!(m.url.to_string(), "https://n/d");
    let m = mirror(&[
      ("NVM_NODEJS_ORG_MIRROR", "https://n/d"),
      ("RNM_NODE_MIRROR", "https://r/d"),
    ])
    .unwrap();
    assert_eq!(m.url.to_string(), "https://r/d");
    let m = mirror(&[
      ("RNM_NODE_MIRROR", ""),
      ("NVM_NODEJS_ORG_MIRROR", "https://n/d"),
    ])
    .unwrap();
    assert_eq!(m.url.to_string(), "https://n/d");
  }

  #[test]
  fn credential_is_scoped_to_the_mirror() {
    let m = mirror(&[
      ("NVM_NODEJS_ORG_MIRROR", "https://mirror.corp/nodejs"),
      ("NVM_AUTH_HEADER", "Bearer abc"),
    ])
    .unwrap();
    let credential = m.credential.unwrap();
    assert_eq!(credential.source, "NVM_AUTH_HEADER");
    assert_eq!(credential.header, AUTHORIZATION);
    assert!(
      credential
        .scope
        .covers(&Url::parse("https://mirror.corp/nodejs/index.json").unwrap())
    );
    assert!(
      !credential
        .scope
        .covers(&Url::parse("https://nodejs.org/dist/index.json").unwrap())
    );
  }

  #[test]
  fn rejects_bad_settings() {
    assert!(matches!(
      mirror(&[("RNM_NODE_MIRROR", "mirror.corp/dist")]),
      Err(MirrorError::InvalidUrl {
        var: "RNM_NODE_MIRROR",
        ..
      })
    ));
    assert_eq!(
      mirror(&[("RNM_AUTH_HEADER", "Bearer a\nX: y")]).unwrap_err(),
      MirrorError::InvalidAuthHeader {
        var: "RNM_AUTH_HEADER"
      }
    );
    assert!(matches!(
      mirror(&[
        ("RNM_NODE_MIRROR", "http://mirror.corp/d"),
        ("FNM_AUTH_HEADER", "Bearer a")
      ]),
      Err(MirrorError::AuthOverPlainHttp {
        var: "FNM_AUTH_HEADER",
        ..
      })
    ));
    assert!(
      mirror(&[
        ("RNM_NODE_MIRROR", "http://127.0.0.1:9/d"),
        ("RNM_AUTH_HEADER", "Bearer a")
      ])
      .is_ok()
    );
  }

  #[test]
  fn errors_never_contain_the_secret() {
    let error = mirror(&[
      ("RNM_NODE_MIRROR", "http://mirror.corp/d"),
      ("RNM_AUTH_HEADER", "Bearer hunter2"),
    ])
    .unwrap_err();
    assert!(!error.to_string().contains("hunter2"));
    assert!(!format!("{error:?}").contains("hunter2"));
  }
}
