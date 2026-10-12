//! HTTP downloads with credentials that never leak.
//!
//! Redirects are followed by rnm, not by the HTTP library, so each hop is
//! checked against the credential's [`Scope`]: a credential is attached only
//! when the target is inside its scope, and is dropped as soon as a redirect
//! leaves it (another host, port, path, or a downgrade to plain http).
//!
//! Proxies come from `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, and
//! `NO_PROXY`. Certificates are checked against the operating system's trust
//! store, so corporate root certificates work without extra settings.

mod auth;
mod mirror;
mod url;

use std::fmt;
use std::io::Read;
use std::time::Duration;

use ureq::http::{StatusCode, header};
use ureq::tls::{RootCerts, TlsConfig};

pub use auth::{Credential, InvalidSecret, Scope, Secret};
pub use mirror::{DEFAULT_NODE_MIRROR, Mirror, MirrorError};
pub use url::{InvalidUrl, Origin, Url};

/// Redirect hops before giving up, the same limit browsers and curl use.
const MAX_REDIRECTS: usize = 10;

/// How long to wait for a TCP and TLS connection. There is no overall
/// timeout, because a large download on a slow link can take minutes.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

const USER_AGENT: &str = concat!("rnm/", env!("CARGO_PKG_VERSION"));

/// A blocking HTTP client.
pub struct Client {
  agent: ureq::Agent,
  credentials: Vec<Credential>,
}

/// A successful response.
pub struct Response {
  /// The URL that answered, after redirects.
  pub url: Url,
  /// The `ETag` header, for conditional requests.
  pub etag: Option<String>,
  /// The body. It is not size-limited; callers decide how much to read.
  pub body: Box<dyn Read + Send>,
}

/// The outcome of a conditional request.
pub enum Conditional {
  /// The resource changed (or there was no `ETag` to compare with).
  Modified(Response),
  /// 304: the copy identified by the `ETag` is still current.
  NotModified,
}

/// Why a request failed.
#[derive(Debug)]
pub enum HttpError {
  /// 401: credentials are missing or were rejected.
  Unauthorized {
    url: String,
    sent_credential_from: Option<String>,
  },
  /// 403: the credentials do not grant access.
  Forbidden {
    url: String,
    sent_credential_from: Option<String>,
  },
  /// 404.
  NotFound { url: String },
  /// Any other non-success status.
  Status { url: String, status: u16 },
  /// More than [`MAX_REDIRECTS`] redirects.
  TooManyRedirects { url: String },
  /// A redirect without a usable `Location`.
  BadRedirect { url: String, location: String },
  /// Network, DNS, TLS, or proxy failure.
  Transport { url: String, message: String },
}

impl fmt::Display for HttpError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let credential_hint = |f: &mut fmt::Formatter<'_>, source: &Option<String>| match source {
      | Some(var) => write!(f, " (sent the credential from {var})"),
      | None => {
        f.write_str(" (no credential was sent; set RNM_AUTH_HEADER, for example 'Bearer <token>')")
      },
    };
    match self {
      | Self::Unauthorized {
        url,
        sent_credential_from,
      } => {
        write!(f, "{url}: 401 Unauthorized")?;
        credential_hint(f, sent_credential_from)
      },
      | Self::Forbidden {
        url,
        sent_credential_from,
      } => {
        write!(f, "{url}: 403 Forbidden")?;
        credential_hint(f, sent_credential_from)
      },
      | Self::NotFound { url } => write!(f, "{url}: 404 Not Found"),
      | Self::Status { url, status } => {
        let reason = StatusCode::from_u16(*status)
          .ok()
          .and_then(|code| code.canonical_reason());
        write!(f, "{url}: HTTP {status} {}", reason.unwrap_or_default())
      },
      | Self::TooManyRedirects { url } => write!(f, "{url}: more than {MAX_REDIRECTS} redirects"),
      | Self::BadRedirect { url, location } => {
        write!(f, "{url}: redirect to an invalid location '{location}'")
      },
      | Self::Transport { url, message } => write!(f, "{url}: {message}"),
    }
  }
}

impl std::error::Error for HttpError {}

impl Client {
  /// Creates a client that sends each of `credentials` only inside its
  /// scope.
  #[must_use]
  pub fn new(credentials: Vec<Credential>) -> Self {
    let config = ureq::Agent::config_builder()
      .max_redirects(0)
      .http_status_as_error(false)
      .timeout_connect(Some(CONNECT_TIMEOUT))
      .user_agent(USER_AGENT)
      .tls_config(
        TlsConfig::builder()
          .root_certs(RootCerts::PlatformVerifier)
          .build(),
      )
      .build();
    Self {
      agent: ureq::Agent::new_with_config(config),
      credentials,
    }
  }

  /// A client for `mirror`, with its credential if it has one.
  #[must_use]
  pub fn for_mirror(mirror: &Mirror) -> Self {
    Self::new(mirror.credential.iter().cloned().collect())
  }

  /// Sends a GET request and follows redirects.
  ///
  /// # Errors
  ///
  /// See [`HttpError`]. Error messages never include credentials or URL
  /// user information.
  pub fn get(&self, url: &Url) -> Result<Response, HttpError> {
    match self.send(url, None)? {
      | Conditional::Modified(response) => Ok(response),
      | Conditional::NotModified => Err(HttpError::Status {
        url: url.to_string(),
        status: StatusCode::NOT_MODIFIED.as_u16(),
      }),
    }
  }

  /// Sends a GET request with `If-None-Match: <etag>` and follows
  /// redirects. Without an `etag` this is a plain [`Client::get`].
  ///
  /// # Errors
  ///
  /// See [`HttpError`].
  pub fn get_if_none_match(&self, url: &Url, etag: Option<&str>) -> Result<Conditional, HttpError> {
    self.send(url, etag)
  }

  fn send(&self, url: &Url, etag: Option<&str>) -> Result<Conditional, HttpError> {
    let mut url = url.clone();
    for _ in 0..=MAX_REDIRECTS {
      let credential = self
        .credentials
        .iter()
        .find(|credential| credential.scope.covers(&url));
      let mut request = self.agent.get(url.uri());
      if let Some(credential) = credential {
        request = request.header(
          credential.header.clone(),
          credential.value.header_value().clone(),
        );
      }
      if let Some(etag) = etag {
        request = request.header(header::IF_NONE_MATCH, etag);
      }
      let response = request.call().map_err(|error| HttpError::Transport {
        url: url.to_string(),
        message: error.to_string(),
      })?;

      let status = response.status();
      let answers_conditional = etag.is_some() && status == StatusCode::NOT_MODIFIED;
      if answers_conditional {
        return Ok(Conditional::NotModified);
      }
      let is_redirect = status.is_redirection() && status != StatusCode::NOT_MODIFIED;
      if is_redirect {
        let location = header_text(&response, header::LOCATION).unwrap_or_default();
        url = url.join(&location).map_err(|_| HttpError::BadRedirect {
          url: url.to_string(),
          location,
        })?;
        continue;
      }

      let sent_credential_from = credential.map(|credential| credential.source.clone());
      return match status {
        | StatusCode::UNAUTHORIZED => Err(HttpError::Unauthorized {
          url: url.to_string(),
          sent_credential_from,
        }),
        | StatusCode::FORBIDDEN => Err(HttpError::Forbidden {
          url: url.to_string(),
          sent_credential_from,
        }),
        | StatusCode::NOT_FOUND => Err(HttpError::NotFound {
          url: url.to_string(),
        }),
        | status if !status.is_success() => Err(HttpError::Status {
          url: url.to_string(),
          status: status.as_u16(),
        }),
        | _ => Ok(Conditional::Modified(Response {
          etag: header_text(&response, header::ETAG),
          body: Box::new(response.into_body().into_reader()),
          url,
        })),
      };
    }
    Err(HttpError::TooManyRedirects {
      url: url.to_string(),
    })
  }
}

fn header_text(
  response: &ureq::http::Response<ureq::Body>,
  name: header::HeaderName,
) -> Option<String> {
  response
    .headers()
    .get(name)?
    .to_str()
    .ok()
    .map(str::to_owned)
}
